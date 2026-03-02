# Refactor: GSD Uses task_queue

## Goal

Make `gsd_config` use `task_queue`'s execution engine. GSD will have a single dynamic task type that implements `QueueItem`.

## Current State

### task_queue

```rust
pub trait QueueItem<Context>: Sized {
    type InProgress;
    type Response: DeserializeOwned;
    type NextTasks;

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command);
    fn process(in_progress: Self::InProgress, result: Result<Self::Response, _>, ctx: &mut Context) -> Self::NextTasks;
}
```

**Problem:** `start()` returns a `Command`, which is executed locally. GSD needs to either:
- Submit to agent_pool and wait for response
- Run a local command with pre/post hooks

### gsd_config

Has its own `TaskRunner` that manages:
- Queue of tasks
- Concurrent execution via thread spawning
- Pre/post/finally hooks
- Retry logic
- Timeout handling

## Proposed Changes

### Change 1: Replace `Command` with `BoxFuture`

Instead of returning a `Command` to execute, return a future that resolves to the response:

```rust
// task_queue/src/lib.rs

pub trait QueueItem<Context>: Sized {
    type InProgress;
    type Response: DeserializeOwned;
    type NextTasks;

    /// Start executing the task. Returns in-progress state and a future that resolves to the response.
    fn start(self, ctx: &mut Context) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Self::Response>>);

    /// Process the result and return follow-up tasks.
    fn process(
        in_progress: Self::InProgress,
        result: TaskOutput<Self::Response>,
        ctx: &mut Context,
    ) -> Self::NextTasks;
}

/// Output from executing a task.
pub enum TaskOutput<T> {
    /// Task completed successfully with deserialized response.
    Success(T),
    /// Task timed out.
    Timeout,
    /// Response failed to deserialize.
    InvalidResponse(serde_json::Error),
    /// I/O or execution error.
    Error(std::io::Error),
}
```

**Why this works:**
- GSD can return a future that submits to agent_pool, waits for response file, parses JSON
- Typed task_queue users can return a future that spawns a local command
- Both use the same `TaskRunner` execution engine

### Change 2: Keep `Command` helper for simple cases

For users who just want to run a local command (the common case for typed task_queue):

```rust
// task_queue/src/lib.rs

/// Helper to create a future from a Command.
pub fn run_command<T: DeserializeOwned>(cmd: Command) -> BoxFuture<'static, TaskOutput<T>> {
    Box::pin(async move {
        let output = TokioCommand::from(cmd)
            .stdout(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                match serde_json::from_slice(&out.stdout) {
                    Ok(val) => TaskOutput::Success(val),
                    Err(e) => TaskOutput::InvalidResponse(e),
                }
            }
            Ok(_) => TaskOutput::Error(io::Error::other("command failed")),
            Err(e) => TaskOutput::Error(e),
        }
    })
}
```

Typed task_queue usage remains clean:

```rust
impl QueueItem<Ctx> for AnalyzeFile {
    // ...
    fn start(self, ctx: &mut Ctx) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Self::Response>>) {
        let mut cmd = Command::new("./analyze.sh");
        cmd.arg(&self.path);
        (AnalyzeInProgress { path: self.path }, run_command(cmd))
    }
}
```

### Change 3: GSD implements `QueueItem` dynamically

```rust
// gsd_config/src/queue_item.rs

impl QueueItem<GsdContext> for Task {
    type InProgress = TaskInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<Task>;

    fn start(self, ctx: &mut GsdContext) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Value>>) {
        let step = ctx.step_for(&self.step).clone();

        // Run pre-hook (synchronously, before spawning)
        let effective_value = match &step.pre {
            Some(pre) => match run_pre_hook(pre, &self.value) {
                Ok(v) => v,
                Err(e) => {
                    // Pre-hook failed - return immediate error
                    return (
                        TaskInProgress::pre_hook_failed(self, e.clone()),
                        Box::pin(async move { TaskOutput::Error(io::Error::other(e)) }),
                    );
                }
            },
            None => self.value.clone(),
        };

        // Build the future based on action type
        let future: BoxFuture<'static, TaskOutput<Value>> = match &step.action {
            Action::Command { script } => {
                run_local_command(script.clone(), effective_value.clone())
            }
            Action::Pool { .. } => {
                submit_to_agent_pool(
                    ctx.pool_path.clone(),
                    self.step.clone(),
                    effective_value.clone(),
                    &step,
                    ctx.config_base_path.clone(),
                )
            }
        };

        (
            TaskInProgress {
                task: self,
                effective_value,
                step,
            },
            future,
        )
    }

    fn process(
        ip: Self::InProgress,
        result: TaskOutput<Value>,
        ctx: &mut GsdContext,
    ) -> Vec<Task> {
        // Handle the result
        let (post_input, raw_tasks) = match result {
            TaskOutput::Success(response) => {
                match validate_response(&response, &ip.step, &ctx.schemas) {
                    Ok(tasks) => (
                        PostHookInput::Success {
                            input: ip.effective_value.clone(),
                            output: response,
                            next: tasks.clone(),
                        },
                        tasks,
                    ),
                    Err(e) => {
                        // Invalid transition or schema violation
                        (
                            PostHookInput::Error {
                                input: ip.effective_value.clone(),
                                error: e.to_string(),
                            },
                            vec![],
                        )
                    }
                }
            }
            TaskOutput::Timeout => (
                PostHookInput::Timeout {
                    input: ip.effective_value.clone(),
                },
                handle_retry_or_drop(&ip.task, ctx, FailureKind::Timeout),
            ),
            TaskOutput::InvalidResponse(e) => (
                PostHookInput::Error {
                    input: ip.effective_value.clone(),
                    error: e.to_string(),
                },
                handle_retry_or_drop(&ip.task, ctx, FailureKind::InvalidResponse),
            ),
            TaskOutput::Error(e) => (
                PostHookInput::Error {
                    input: ip.effective_value.clone(),
                    error: e.to_string(),
                },
                vec![],
            ),
        };

        // Run post-hook if configured
        let final_tasks = match &ip.step.post {
            Some(post) => match run_post_hook(post, &post_input) {
                Ok(modified) => extract_next_tasks(&modified),
                Err(_) => raw_tasks, // Post hook failed, use original tasks
            },
            None => raw_tasks,
        };

        final_tasks
    }
}
```

## Hooks: GSD-Specific, Not task_queue

| Hook | Where it runs | Responsibility |
|------|---------------|----------------|
| **pre** | In `start()`, before returning future | GSD only |
| **post** | In `process()`, after getting result | GSD only |
| **finally** | After all descendants complete | GSD only (see below) |

task_queue doesn't need to know about hooks. They're implementation details inside GSD's `QueueItem` impl.

## Finally Hooks: The Hard Part

### Current Implementation

GSD tracks "origin" tasks. When a task with `finally` spawns children:
1. Children get `origin_id = parent.id`
2. `finally_tracking[parent.id].pending_count = num_children`
3. As each descendant completes, decrement count
4. When count hits 0, run the finally hook

This requires:
- Per-task ID tracking
- A `finally_tracking: HashMap<TaskId, FinallyState>` in the runner
- Propagating `origin_id` through spawned tasks

### Problem

task_queue's `TaskRunner` doesn't have this concept. It just runs tasks and collects results.

### Options

**Option A: Add finally support to task_queue**

Add an optional `FinallyTracker` that task_queue maintains:

```rust
pub trait QueueItem<Context>: Sized {
    // ... existing methods ...

    /// Optional: called when all tasks spawned by this task have completed.
    /// Default: no-op.
    fn finally(in_progress: &Self::InProgress, ctx: &mut Context) -> Self::NextTasks {
        Default::default()
    }

    /// Does this task have a finally handler?
    fn has_finally(&self, ctx: &Context) -> bool {
        false
    }
}
```

task_queue's `TaskRunner` would need to track parent-child relationships internally.

**Option B: GSD wraps task_queue's TaskRunner**

GSD uses task_queue for execution but adds its own layer for finally tracking:

```rust
// gsd_config uses task_queue::TaskRunner internally but wraps it

struct GsdRunner {
    inner: task_queue::TaskRunner<Task, TaskInProgress, GsdContext>,
    finally_tracking: HashMap<TaskId, FinallyState>,
}
```

When `inner.next()` returns, GSD:
1. Checks if the completed task has an origin
2. Decrements the origin's pending count
3. Runs finally hook if count == 0

**Option C: Model finally as a separate task**

Instead of hooks, `finally` becomes a "FinallyWait" task that blocks until all children complete:

```rust
// When Distribute spawns Worker tasks, it also spawns:
Task::FinallyWait {
    waiting_for: vec![worker1_id, worker2_id, ...],
    then_run: "cleanup.sh",
}
```

This makes the state machine explicit but complicates the config format.

### Recommendation

**Option B** - GSD wraps task_queue's runner. Reasons:
1. Keeps task_queue simple (it doesn't need to understand finally semantics)
2. GSD already has this logic; we just move it to a wrapper layer
3. Finally is GSD-specific anyway (typed task_queue users can model it differently)

## GsdContext

Consolidate GSD's scattered state into a single context:

```rust
pub struct GsdContext {
    pub config: Config,
    pub schemas: CompiledSchemas,
    pub pool_path: PathBuf,
    pub config_base_path: PathBuf,
}

impl GsdContext {
    pub fn step_for(&self, name: &StepName) -> Option<&Step> {
        self.config.step_map().get(name.as_str()).copied()
    }

    pub fn effective_options(&self, step: &Step) -> EffectiveOptions {
        EffectiveOptions::resolve(&self.config.options, &step.options)
    }
}
```

## Implementation Plan

### Task 1: Modify task_queue's QueueItem trait

Change `start()` to return `BoxFuture` instead of `Command`:

```rust
fn start(self, ctx: &mut Context) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Self::Response>>);
```

Add `TaskOutput` enum with `Success`, `Timeout`, `InvalidResponse`, `Error` variants.

Add `run_command()` helper for backward compatibility.

### Task 2: Update task_queue's TaskRunner

Modify execution loop to `.await` the future instead of spawning a `Command`.

### Task 3: Create GsdContext

Consolidate config, schemas, paths into single struct.

### Task 4: Implement QueueItem for Task in gsd_config

- `start()`: run pre-hook, return future for either pool submission or local command
- `process()`: validate response, run post-hook, return next tasks

### Task 5: Create GSD wrapper for finally tracking

Wrap task_queue's `TaskRunner` with origin/finally tracking logic.

### Task 6: Replace gsd_config's TaskRunner

Delete the current `runner.rs` implementation, use task_queue's runner with the wrapper.

### Task 7: Make gsd_config async

Add tokio dependency, make `run()` async.

## TODOs

- [ ] **Add tests for `finally` hooks** - Currently no tests or demos exercise this feature
- [ ] **Add demo for `finally`** - Show a fan-out that aggregates results
- [ ] **Document `finally` behavior** - When it runs, what input it receives, error handling

## Current `finally` Implementation (for reference)

From `gsd_config/src/runner.rs`:

```rust
/// State for tracking when a `finally` hook should run.
struct FinallyState {
    /// Number of descendants still pending (in queue or in flight).
    pending_count: usize,
    /// The original task's value (input to finally hook).
    original_value: serde_json::Value,
    /// The finally hook command.
    finally_command: String,
}
```

When a task completes:
1. If it spawned children and has `finally`, register in `finally_tracking` with count = num_children
2. Children inherit `origin_id` pointing to parent
3. On each descendant completion, decrement `finally_tracking[origin_id].pending_count`
4. When count == 0, run `finally_command` with original value on stdin
5. Finally output (JSON array) spawns new tasks (without origin tracking)

**Edge cases:**
- Finally runs even if descendants failed
- Finally failures are logged but ignored
- Finally-spawned tasks don't inherit origin (prevents infinite tracking)

## Summary

| Component | Responsibility |
|-----------|----------------|
| **task_queue** | Queue execution, concurrency, async futures |
| **gsd_config** | Config parsing, validation, hooks, finally tracking |
| **QueueItem impl** | Bridge between them - GSD's Task implements task_queue's trait |

The key insight: task_queue provides the execution engine, GSD provides the dynamic/config-driven behavior. They compose via the `QueueItem` trait, with GSD returning futures that do whatever GSD needs (pool submission, hooks, etc.).

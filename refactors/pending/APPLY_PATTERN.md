# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** STATE_PERSISTENCE (should be implemented as part of or after Phase 3)

## Motivation

Currently, state changes and log writes are separate operations:

```rust
// Two separate operations - easy to forget one
self.log_writer.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates risk of:
1. Forgetting to write to log when changing state
2. Log/state getting out of sync
3. Bugs where state is updated but log isn't (or vice versa)

## Core Idea

All state changes go through a single `apply()` method that atomically writes to log AND updates internal state:

```rust
impl TaskRunner {
    /// All state changes go through this. Writes to log, then updates internal state.
    fn apply(&mut self, entry: StateLogEntry) {
        // 1. Write to log (guaranteed to happen)
        self.log_writer.write(&entry);

        // 2. Update internal state based on entry
        match entry {
            StateLogEntry::TaskSubmitted(submitted) => {
                self.apply_task_submitted(submitted);
            }
            StateLogEntry::TaskCompleted(completed) => {
                self.apply_task_completed(completed);
            }
            StateLogEntry::Config(_) => {
                // Config is only written once at start, no state update needed
            }
        }
    }
}
```

## Benefits

1. **Impossible to forget logging** - state only changes through `apply()`, which always logs
2. **Resume uses same code path** - replay log entries through `apply()` to reconstruct state
3. **Single source of truth** - log entries define what state changes are possible
4. **Testable** - can unit test `apply()` in isolation with fake log writer

## Design Challenges

### Challenge 1: Derived State

Some state doesn't map cleanly to log entries:
- `in_flight` counter (number of tasks currently executing)
- `next_task_id` (monotonic counter)

**Options:**

A. **Track in log entry** - Add fields like `dispatched: bool` to TaskSubmitted
B. **Derive from state** - Recompute `in_flight` by counting InFlight states
C. **Separate from apply** - Only use apply for logged state, keep counters separate

**Recommendation:** Option B (derive from state) for `in_flight`, Option C for `next_task_id` (it's derived from max task_id in log).

### Challenge 2: TaskState Transitions

Current code has complex state transitions:
- `Pending` → `InFlight` (on dispatch)
- `InFlight` → `WaitingForChildren` (on success with children)
- `InFlight` → removed (on success without children)
- `WaitingForChildren` → removed (when all children complete)

The log only captures:
- `TaskSubmitted` (creates Pending)
- `TaskCompleted` (removes task or transitions to WaitingForChildren)

**Solution:** `apply_task_completed` handles the complex logic:

```rust
fn apply_task_completed(&mut self, completed: TaskCompleted) {
    match completed.outcome {
        TaskOutcome::Success(success) => {
            if success.spawned_task_ids.is_empty() {
                // No children - task is done
                self.tasks.remove(&completed.task_id);
                self.notify_parent(completed.task_id);
            } else {
                // Has children - transition to WaitingForChildren
                let entry = self.tasks.get_mut(&completed.task_id).expect("task exists");
                entry.state = TaskState::WaitingForChildren {
                    pending_children_count: NonZeroU16::new(success.spawned_task_ids.len() as u16).unwrap(),
                    finally_data: /* ... */,
                };
            }
        }
        TaskOutcome::Failed(failed) => {
            if failed.retry_task_id.is_some() {
                // Retry will be logged separately as TaskSubmitted
                self.tasks.remove(&completed.task_id);
            } else {
                // Permanent failure
                self.tasks.remove(&completed.task_id);
                // Handle finally, notify parent, etc.
            }
        }
    }
}
```

### Challenge 3: Dispatch is Not Logged

Dispatching a task (Pending → InFlight) is not a logged event. Options:

A. **Log dispatch** - Add `TaskDispatched { task_id }` entry
B. **Don't track InFlight in state** - Only track Pending and WaitingForChildren
C. **Keep dispatch outside apply** - Only logged events go through apply

**Recommendation:** Option C. The InFlight state is transient and doesn't need to survive resume (we re-dispatch pending tasks on resume anyway).

```rust
// dispatch() is separate from apply() - it only changes InFlight status
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task exists");
    let value = match &entry.state {
        TaskState::Pending { value } => value.clone(),
        _ => panic!("can only dispatch pending tasks"),
    };
    entry.state = TaskState::InFlight(/* ... */);
    self.spawn_task_thread(task_id, value);
}
```

## Implementation

### Before/After: queue_task

```rust
// BEFORE
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();

    self.log_writer.write(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value.0.clone(),
        parent_id,
        origin,
    }));

    self.tasks.insert(id, TaskEntry { /* ... */ });

    if self.in_flight < self.max_concurrency {
        self.dispatch(id);
    }
}

// AFTER
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();

    // Single call handles both logging AND state update
    self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value.0.clone(),
        parent_id,
        origin,
    }));

    // Dispatch is separate (not logged)
    if self.in_flight() < self.max_concurrency {
        self.dispatch(id);
    }
}
```

### Before/After: task_succeeded

```rust
// BEFORE
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;

    // ... complex logic ...

    self.log_writer.write(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids }),
    }));

    // ... more state updates ...
}

// AFTER
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    // Queue children first (each gets its own TaskSubmitted)
    let spawned_task_ids: Vec<LogTaskId> = spawned.iter().map(|task| {
        let id = self.next_task_id();
        self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step.clone(),
            value: task.value.0.clone(),
            parent_id: Some(task_id),
            origin: TaskOrigin::Spawned,
        }));
        id
    }).collect();

    // Then complete the parent (apply handles WaitingForChildren transition)
    self.apply(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids }),
    }));
}
```

### New: apply_task_submitted

```rust
fn apply_task_submitted(&mut self, submitted: TaskSubmitted) {
    let step = self.step_map.get(&submitted.step).expect("step exists");
    let retries_remaining = step.options.max_retries;

    let finally_script = if matches!(submitted.origin, TaskOrigin::Finally { .. }) {
        step.finally.clone()
    } else {
        None
    };

    self.tasks.insert(submitted.task_id, TaskEntry {
        step: submitted.step,
        parent_id: submitted.parent_id,
        finally_script,
        state: TaskState::Pending { value: StepInputValue(submitted.value) },
        retries_remaining,
    });

    // Update parent's pending_children_count if needed
    if let Some(parent_id) = submitted.parent_id {
        self.increment_pending_children(parent_id);
    }
}
```

### New: apply_task_completed

```rust
fn apply_task_completed(&mut self, completed: TaskCompleted) {
    let entry = self.tasks.get(&completed.task_id).expect("task exists");
    let parent_id = entry.parent_id;
    let finally_hook = self.lookup_finally_hook(entry);

    match completed.outcome {
        TaskOutcome::Success(success) => {
            if success.spawned_task_ids.is_empty() {
                // No children - schedule finally if needed, then remove
                if let Some(hook) = finally_hook {
                    let value = entry.get_value().clone();
                    // Note: schedule_finally will call apply() for the finally task
                    self.schedule_finally(completed.task_id, hook, value);
                }
                self.tasks.remove(&completed.task_id);
                if let Some(parent_id) = parent_id {
                    self.decrement_pending_children(parent_id);
                }
            } else {
                // Has children - transition to WaitingForChildren
                let entry = self.tasks.get_mut(&completed.task_id).unwrap();
                let finally_data = finally_hook.map(|hook| (hook, entry.get_value().clone()));
                entry.state = TaskState::WaitingForChildren {
                    pending_children_count: NonZeroU16::new(success.spawned_task_ids.len() as u16).unwrap(),
                    finally_data,
                };
            }
        }
        TaskOutcome::Failed(failed) => {
            self.tasks.remove(&completed.task_id);
            if failed.retry_task_id.is_none() {
                // Permanent failure - notify parent
                if let Some(parent_id) = parent_id {
                    self.decrement_pending_children(parent_id);
                }
            }
            // If retry_task_id is Some, the retry TaskSubmitted handles parent relationship
        }
    }
}
```

## Resume Integration

With apply pattern, resume becomes trivial:

```rust
fn resume_from_log(&mut self, entries: impl Iterator<Item = StateLogEntry>) {
    for entry in entries {
        match entry {
            StateLogEntry::Config(_) => {
                // Already handled during initialization
            }
            _ => {
                // Don't write to log (we're replaying), just apply state change
                self.apply_without_logging(entry);
            }
        }
    }
}

fn apply_without_logging(&mut self, entry: StateLogEntry) {
    match entry {
        StateLogEntry::TaskSubmitted(submitted) => {
            self.apply_task_submitted(submitted);
        }
        StateLogEntry::TaskCompleted(completed) => {
            self.apply_task_completed(completed);
        }
        StateLogEntry::Config(_) => {}
    }
}
```

Or refactor `apply()` to take a flag:

```rust
fn apply(&mut self, entry: StateLogEntry, write_to_log: bool) {
    if write_to_log {
        self.log_writer.write(&entry);
    }
    // ... same state update logic ...
}
```

## Testing

```rust
#[test] fn apply_task_submitted_creates_pending_entry()
#[test] fn apply_task_submitted_increments_parent_children()
#[test] fn apply_task_completed_success_no_children_removes()
#[test] fn apply_task_completed_success_with_children_waits()
#[test] fn apply_task_completed_failed_with_retry_removes()
#[test] fn apply_task_completed_failed_no_retry_notifies_parent()
#[test] fn apply_sequence_matches_manual_state_changes()
#[test] fn replay_log_reconstructs_identical_state()
```

## Migration Path

1. Implement `apply_task_submitted` and `apply_task_completed` as new methods
2. Refactor existing code to call `apply()` instead of direct mutations
3. Verify all tests pass
4. Remove dead code paths that did direct mutations

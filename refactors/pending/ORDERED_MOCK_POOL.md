# Ordered Mock Pool for Deterministic Tests

**Status:** Not started

## Motivation

Current tests use `GsdTestAgent` with `processing_delay` (time-based delays) to simulate agent response times. This works for basic tests but fails for deterministic snapshot testing:

1. **Fan-out tests are non-deterministic** - When multiple tasks run concurrently, completion order depends on timing
2. **Snapshot tests require stable output** - State logs must be identical across runs
3. **Complex scenarios need controlled ordering** - Testing retry + finally + fan-out interactions requires precise control

## Current Test Infrastructure

`GsdTestAgent` in `crates/gsd_cli/tests/common/mod.rs`:

```rust
impl GsdTestAgent {
    pub fn start<F>(root: &Path, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        // ... spawns thread that:
        // 1. Waits for task assignment via inotify
        // 2. Sleeps for processing_delay
        // 3. Calls processor(payload) to get response
        // 4. Writes response
    }

    pub fn terminator(root: &Path, processing_delay: Duration) -> Self { ... }
    pub fn transition_to(root: &Path, processing_delay: Duration, next_kind: &str) -> Self { ... }
    pub fn with_transitions(root: &Path, processing_delay: Duration, transitions: Vec<(&str, &str)>) -> Self { ... }
}
```

The `processor` closure generates responses synchronously. Time-based ordering via `processing_delay` is inherently racy.

## Proposed Solution

Add `OrderedAgentController` that lets tests explicitly control when each task completes.

### New Types

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A waiting task that hasn't been completed yet.
pub struct WaitingTask {
    /// The task kind (e.g., "Worker", "Analyze")
    pub kind: String,
    /// The full payload JSON
    pub payload: String,
    /// Channel to send the response
    response_tx: oneshot::Sender<String>,
}

/// Controller for completing tasks in any order.
///
/// Tasks register themselves when they arrive, and tests can complete
/// them in any order by specifying which task to complete.
pub struct OrderedAgentController {
    /// Tasks waiting for completion, keyed by arrival order.
    waiting: Arc<Mutex<Vec<WaitingTask>>>,
    /// Notifier for when new tasks arrive.
    task_arrived: Arc<(Mutex<()>, Condvar)>,
}

impl OrderedAgentController {
    /// Wait for a task with the given kind to arrive.
    /// Returns the index of the task in the waiting queue.
    pub fn wait_for(&self, kind: &str) -> usize {
        loop {
            {
                let waiting = self.waiting.lock().unwrap();
                if let Some(idx) = waiting.iter().position(|t| t.kind == kind) {
                    return idx;
                }
            }
            // Wait for notification that a new task arrived
            let (lock, cvar) = &*self.task_arrived;
            let guard = lock.lock().unwrap();
            let _ = cvar.wait_timeout(guard, Duration::from_millis(100));
        }
    }

    /// Wait for any task to arrive. Returns its index.
    pub fn wait_for_any(&self) -> usize {
        loop {
            {
                let waiting = self.waiting.lock().unwrap();
                if !waiting.is_empty() {
                    return waiting.len() - 1;  // Most recent
                }
            }
            let (lock, cvar) = &*self.task_arrived;
            let guard = lock.lock().unwrap();
            let _ = cvar.wait_timeout(guard, Duration::from_millis(100));
        }
    }

    /// Get list of currently waiting tasks (kind, payload).
    pub fn waiting_tasks(&self) -> Vec<(String, String)> {
        let waiting = self.waiting.lock().unwrap();
        waiting.iter().map(|t| (t.kind.clone(), t.payload.clone())).collect()
    }

    /// Complete a specific waiting task by index.
    pub fn complete_at(&self, index: usize, response: &str) {
        let task = {
            let mut waiting = self.waiting.lock().unwrap();
            waiting.remove(index)
        };
        let _ = task.response_tx.send(response.to_string());
    }

    /// Complete the first waiting task with the given kind.
    pub fn complete(&self, kind: &str, response: &str) {
        let idx = self.wait_for(kind);
        self.complete_at(idx, response);
    }

    /// Complete a task with empty array (terminate, no children).
    pub fn terminate(&self, kind: &str) {
        self.complete(kind, "[]");
    }

    /// Complete a task, spawning child tasks.
    pub fn spawn(&self, kind: &str, children: &[&str]) {
        let tasks: Vec<String> = children
            .iter()
            .map(|k| format!(r#"{{"kind": "{k}", "value": {{}}}}"#))
            .collect();
        self.complete(kind, &format!("[{}]", tasks.join(", ")));
    }

    /// Complete all waiting tasks of a given kind with the same response.
    pub fn complete_all(&self, kind: &str, response: &str) {
        loop {
            let idx = {
                let waiting = self.waiting.lock().unwrap();
                waiting.iter().position(|t| t.kind == kind)
            };
            match idx {
                Some(i) => self.complete_at(i, response),
                None => break,
            }
        }
    }

    /// Terminate all waiting tasks of a given kind.
    pub fn terminate_all(&self, kind: &str) {
        self.complete_all(kind, "[]");
    }
}
```

### New GsdTestAgent Method

```rust
impl GsdTestAgent {
    /// Start an agent that waits for explicit completion signals.
    ///
    /// Tasks register with the controller when they arrive and block
    /// until the test explicitly completes them. Tests can complete
    /// tasks in any order, not just FIFO.
    ///
    /// Returns (agent, controller).
    pub fn ordered(root: &Path) -> (Self, OrderedAgentController) {
        let waiting: Arc<Mutex<Vec<WaitingTask>>> = Arc::new(Mutex::new(Vec::new()));
        let task_arrived = Arc::new((Mutex::new(()), Condvar::new()));

        let waiting_clone = waiting.clone();
        let task_arrived_clone = task_arrived.clone();

        let agent = Self::start(root, Duration::ZERO, move |payload| {
            // Parse task kind from payload
            let kind = serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .and_then(|v| v.get("task")?.get("kind")?.as_str().map(String::from))
                .unwrap_or_else(|| "Unknown".to_string());

            // Create oneshot channel for this task's response
            let (tx, rx) = oneshot::channel();

            // Register as waiting
            {
                let mut waiting = waiting_clone.lock().unwrap();
                waiting.push(WaitingTask {
                    kind,
                    payload: payload.to_string(),
                    response_tx: tx,
                });
            }

            // Notify controller that a task arrived
            {
                let (_, cvar) = &*task_arrived_clone;
                cvar.notify_all();
            }

            // Block until test sends response
            rx.recv().unwrap_or_else(|_| "[]".to_string())
        });

        let controller = OrderedAgentController {
            waiting,
            task_arrived,
        };

        (agent, controller)
    }
}
```

### Usage Example

```rust
#[test]
fn fan_out_deterministic_order() {
    let root = setup_test_dir("fan_out_ordered");
    let pool = AgentPoolHandle::start(&root.join("pool"));
    let (agent, ctrl) = GsdTestAgent::ordered(&root.join("pool"));

    // Config: Distribute -> Worker (fan-out)
    let config = r#"{ "steps": [
        {"name": "Distribute", "action": {"kind": "Pool", ...}, "next": ["Worker"]},
        {"name": "Worker", "action": {"kind": "Pool", ...}, "next": []}
    ]}"#;

    let gsd = GsdRunner::new();

    // Start GSD in background
    let handle = thread::spawn(move || {
        gsd.run(config, r#"[{"kind": "Distribute", "value": {}}]"#, &root.join("pool"))
    });

    // Wait for Distribute task, complete it with 3 workers
    ctrl.spawn("Distribute", &["Worker", "Worker", "Worker"]);

    // Wait for all 3 workers to arrive
    ctrl.wait_for("Worker");
    ctrl.wait_for("Worker");
    ctrl.wait_for("Worker");

    // Now we have 3 Worker tasks waiting - complete them in reverse order!
    // (or any order we want for testing)
    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 3);

    // Complete in specific order (e.g., by index)
    ctrl.complete_at(2, "[]");  // Third worker first
    ctrl.complete_at(1, "[]");  // Second worker
    ctrl.complete_at(0, "[]");  // First worker last

    let output = handle.join().unwrap();
    // State log has deterministic ordering based on completion order
}

#[test]
fn fan_out_complete_by_kind() {
    // ... setup ...

    // Simpler API when you don't care about specific ordering within a kind
    ctrl.spawn("Distribute", &["Worker", "Worker", "Worker"]);

    // Complete all workers (order within kind is FIFO)
    ctrl.terminate("Worker");
    ctrl.terminate("Worker");
    ctrl.terminate("Worker");
}
```

## Implementation Phases

### Phase 1: Add OrderedAgentController

**File:** `crates/gsd_cli/tests/common/mod.rs`

1. Add `OrderedAgentController` struct with `complete()`, `terminate()`, `spawn_one()`, `spawn()` methods
2. Add `GsdTestAgent::ordered(root) -> (Self, OrderedAgentController)`
3. Add basic test verifying ordered completion works

**Tests:**
```rust
#[test] fn ordered_agent_single_task()
#[test] fn ordered_agent_complete_by_kind()
#[test] fn ordered_agent_complete_at_index()
#[test] fn ordered_agent_wait_for_specific_kind()
#[test] fn ordered_agent_complete_out_of_order()
#[test] fn ordered_agent_complete_all_of_kind()
#[test] fn ordered_agent_waiting_tasks_query()
```

### Phase 2: Add Payload-Aware Completion

The basic `ordered()` ignores the payload. For more control, add payload inspection:

```rust
impl GsdTestAgent {
    /// Start an ordered agent that exposes received payloads.
    ///
    /// Returns (agent, controller) where controller can inspect payloads.
    pub fn ordered_with_payloads(root: &Path) -> (Self, PayloadAwareController) {
        // Implementation uses two channels:
        // 1. payload_tx: agent -> test (sends payload when task arrives)
        // 2. response_rx: test -> agent (receives response to send)
    }
}

pub struct PayloadAwareController {
    payload_rx: mpsc::Receiver<String>,
    response_tx: mpsc::Sender<String>,
}

impl PayloadAwareController {
    /// Wait for next task and return its payload.
    pub fn next_payload(&self) -> String {
        self.payload_rx.recv().expect("agent dropped")
    }

    /// Complete the current task with this response.
    pub fn complete(&self, response: &str) {
        self.response_tx.send(response.to_string()).expect("agent dropped");
    }

    /// Wait for task, inspect payload, then complete.
    pub fn handle<F>(&self, f: F)
    where
        F: FnOnce(&str) -> String,
    {
        let payload = self.next_payload();
        let response = f(&payload);
        self.complete(&response);
    }
}
```

**Tests:**
```rust
#[test] fn payload_aware_inspects_task_kind()
#[test] fn payload_aware_conditional_response()
```

### Phase 3: Update Demos for Deterministic Testing

Create test wrappers for existing demos that use ordered completion:

**File:** `crates/gsd_cli/tests/demo_deterministic.rs`

```rust
/// Run fan-out demo with controlled ordering.
///
/// Order: Distribute completes, then workers complete in ID order.
#[test]
fn demo_fan_out_deterministic() {
    // Use same config as demos/fan-out/config.jsonc
    // Use OrderedAgentController to control completion order
    // Assert state log matches snapshot
}

/// Run fan-out demo with reverse worker completion.
///
/// Tests that finally still works correctly regardless of child order.
#[test]
fn demo_fan_out_reverse_order() {
    // Same config, but complete workers in reverse order
}

/// Run branching demo with deterministic path.
#[test]
fn demo_branching_approve_path() {
    // Control which branch is taken
}

#[test]
fn demo_branching_reject_path() {
    // Control other branch
}
```

### Phase 4: Snapshot Testing Infrastructure

Add snapshot comparison utilities:

```rust
/// Compare state log against expected snapshot.
fn assert_log_matches_snapshot(log_path: &Path, snapshot_name: &str) {
    let actual = fs::read_to_string(log_path).unwrap();
    let expected_path = Path::new("tests/snapshots").join(format!("{snapshot_name}.ndjson"));

    if env::var("UPDATE_SNAPSHOTS").is_ok() {
        fs::write(&expected_path, &actual).unwrap();
        return;
    }

    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|_| panic!("snapshot not found: {}", expected_path.display()));

    assert_eq!(actual, expected, "state log differs from snapshot");
}
```

**Snapshots to create:**
- `tests/snapshots/fan_out_forward.ndjson`
- `tests/snapshots/fan_out_reverse.ndjson`
- `tests/snapshots/branching_approve.ndjson`
- `tests/snapshots/branching_reject.ndjson`
- `tests/snapshots/linear.ndjson`
- `tests/snapshots/hooks.ndjson`

## Testing the Refactor

After each phase:
1. `cargo test -p gsd_cli` - all existing tests still pass
2. New deterministic tests pass consistently (run 10x to verify no flakiness)

## Future Work

- Timeout handling for ordered agents (detect test bugs that forget to complete)
- Multi-agent ordered pools (multiple agents, controlled interleaving)
- Record/replay mode (record actual completion order, replay for debugging)

# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

The state file IS the internal task queue state. Completed tasks are append-only with unique IDs for debugging/tracking.

## State Structure

```rust
// crates/gsd_config/src/queue_state.rs

use crate::types::StepName;
use serde::{Deserialize, Serialize};

/// Unique identifier for a task instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(u64);

/// The task queue state. This is both the runtime state AND the serialization format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueState {
    /// Counter for generating unique task IDs.
    next_id: u64,
    /// Tasks waiting to be processed.
    pub pending: Vec<PendingTask>,
    /// Tasks that completed (append-only log).
    pub completed: Vec<CompletedTask>,
}

/// A task waiting to be processed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTask {
    pub id: TaskId,
    pub step: StepName,
    pub value: serde_json::Value,
    pub retries: u32,
}

/// A completed task (append-only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedTask {
    pub id: TaskId,
    pub step: StepName,
    pub value: serde_json::Value,
    /// IDs of tasks spawned by this task's completion.
    pub spawned: Vec<TaskId>,
}

impl QueueState {
    pub fn new() -> Self {
        Self {
            next_id: 0,
            pending: Vec::new(),
            completed: Vec::new(),
        }
    }

    fn next_id(&mut self) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Create state from initial tasks.
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        let mut state = Self::new();
        for task in tasks {
            state.enqueue_one(task);
        }
        state
    }

    /// Add a task to the pending queue.
    pub fn enqueue_one(&mut self, task: Task) {
        let id = self.next_id();
        self.pending.push(PendingTask {
            id,
            step: task.step,
            value: task.value,
            retries: task.retries,
        });
    }

    /// Add multiple tasks to the pending queue.
    pub fn enqueue(&mut self, tasks: Vec<Task>) {
        for task in tasks {
            self.enqueue_one(task);
        }
    }

    /// Mark a task as completed (moves from pending to completed).
    pub fn complete(&mut self, id: &TaskId) {
        if let Some(pos) = self.pending.iter().position(|t| &t.id == id) {
            let task = self.pending.remove(pos);
            self.completed.push(CompletedTask {
                id: task.id,
                step: task.step,
                value: task.value,
            });
        }
    }
}
```

## State File Location

Default: `<root>/runs/<pool>.<run-id>.json`

Example: `/tmp/agent_pool/runs/mypool.a3f2c1.json`

The run ID is generated at start (short UUID). Multiple runs for the same pool tracked separately.

## Implementation Phases

### Phase 1: Internal State Representation

Change `TaskRunner` to use `QueueState` internally.

**Changes:**
- Add `QueueState`, `PendingTask`, `CompletedTask`, `TaskId` types
- Modify `TaskRunner` to own a `QueueState`
- Replace `VecDeque<QueuedTask>` with `state.pending`
- When task completes: call `state.complete(id)`
- When new tasks spawn: call `state.enqueue(tasks)`

**No CLI changes** - internal refactor only. Tests should pass unchanged.

### Phase 2: Deserialize Initial State into QueueState

Make `--initial-state` and `--entrypoint-value` flow through `QueueState`.

**Changes:**
- `RunnerConfig.initial_tasks: Vec<Task>` → `RunnerConfig.state: QueueState`
- `resolve_initial_tasks()` → `resolve_initial_state()` returning `QueueState`
- For `--initial-state '[...]'`: parse as `Vec<Task>`, convert via `QueueState::from_tasks()`
- For `--entrypoint-value '{}'`: create single `Task`, convert via `QueueState::from_tasks()`

**Flow:**
```
--initial-state '[...]' or --entrypoint-value '{...}'
        ↓
    Vec<Task>
        ↓
    QueueState::from_tasks(tasks)
        ↓
    TaskRunner::new(state)
```

### Phase 3: State Serialization/Deserialization

Add `--state-output` and ability to resume from state file.

**Changes:**
- Add `--state-output <path>` CLI flag
- After each task completion, serialize `QueueState` to file
- Delete state file on successful completion
- Modify `resolve_initial_state()` to detect and parse `QueueState` directly from file

**Detecting state file vs task array:**
- If file contains `{"pending": [...], "completed": [...]}` → it's a `QueueState`
- If file contains `[{"kind": ..., "value": ...}]` → it's a `Vec<Task>`

**Flow for resume:**
```
--initial-state /path/to/state.json
        ↓
    parse file → detect QueueState format
        ↓
    TaskRunner::new(state)
        ↓
    continue from where we left off
```

## CLI

```bash
# Normal run
gsd run config.jsonc --pool mypool --initial-state '[{"kind": "Start", "value": {}}]'

# Run with state output
gsd run config.jsonc --pool mypool --initial-state '[...]' --state-output /tmp/run.state.json

# Resume from state file
gsd run config.jsonc --pool mypool --initial-state /tmp/run.state.json
```

## What We Don't Track (v1)

- **In-flight tasks**: On resume, tasks being processed are lost. May cause duplicate work.
- **Finally hook state**: On resume, finally hooks won't fire correctly if mid-fan-out.

## Future Work (TODOs)

Add to todos.md:

### List Resume Files

```bash
gsd runs list --root /tmp/agent_pool
# Shows: mypool.a3f2c1.json (3 pending, 7 completed)
```

### Config Hash Validation

Store config hash in state file. On resume, validate config hasn't changed.

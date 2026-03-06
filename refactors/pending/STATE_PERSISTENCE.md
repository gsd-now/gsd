# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (state files will live under `--root/state/`)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

The state file IS the internal task queue state. No separate "persisted" types - we serialize the actual data structure we use at runtime.

## State Structure

This is the runtime state we track and serialize:

```rust
// crates/gsd_config/src/queue_state.rs

use crate::value_schema::Task;
use serde::{Deserialize, Serialize};

/// The task queue state. This is both the runtime state AND the serialization format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueState {
    /// Tasks waiting to be processed.
    pub pending: Vec<Task>,
    /// Tasks that completed successfully (step name + value for debugging).
    pub completed: Vec<Task>,
}

impl QueueState {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            completed: Vec::new(),
        }
    }

    pub fn from_initial_tasks(tasks: Vec<Task>) -> Self {
        Self {
            pending: tasks,
            completed: Vec::new(),
        }
    }
}
```

The existing `Task` type already has everything we need:
```rust
pub struct Task {
    pub step: StepName,
    pub value: serde_json::Value,
    pub retries: u32,
}
```

## Flow

### Current flow (without persistence)

```
--initial-state '[...]' or --entrypoint-value '{...}'
        ↓
    Vec<Task>
        ↓
    TaskRunner::new(initial_tasks)
        ↓
    run loop
```

### New flow (with persistence)

```
--initial-state '[...]' or --entrypoint-value '{...}' or state.json
        ↓
    QueueState { pending: [...], completed: [] }
        ↓
    TaskRunner::new(state)
        ↓
    run loop (updates state.pending, state.completed)
        ↓
    if --state-output: serialize QueueState to file
```

The key insight: `--initial-state` and `--entrypoint-value` are just ways to construct an initial `QueueState`. A state file is another way. They all produce the same data structure.

## CLI

```bash
# Normal run - constructs QueueState from initial tasks
gsd run config.jsonc --pool mypool --initial-state '[{"kind": "Start", "value": {}}]'

# Run with state output - saves QueueState to file on each completion
gsd run config.jsonc --pool mypool --initial-state '[...]' --state-output /tmp/run.state.json

# Resume from state file - loads QueueState from file
gsd run config.jsonc --pool mypool --initial-state /tmp/run.state.json
```

`--initial-state` already detects file vs inline JSON by checking if the path exists.

## Code Changes

### 1. Add QueueState type

**File:** `crates/gsd_config/src/queue_state.rs` (new)

```rust
use crate::value_schema::Task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueState {
    pub pending: Vec<Task>,
    pub completed: Vec<Task>,
}

impl QueueState {
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self {
            pending: tasks,
            completed: Vec::new(),
        }
    }
}
```

### 2. Modify TaskRunner to use QueueState

**File:** `crates/gsd_config/src/runner.rs`

```rust
pub struct RunnerConfig<'a> {
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state: QueueState,  // Changed from initial_tasks: Vec<Task>
    pub invoker: &'a Invoker<AgentPoolCli>,
    pub state_output: Option<&'a Path>,
}
```

TaskRunner holds a reference to the state and updates it:
- When task completes: remove from pending, add to completed
- `snapshot()` returns clone of current state for serialization

### 3. Modify parse_initial_tasks to return QueueState

**File:** `crates/gsd_cli/src/main.rs`

```rust
fn parse_initial_state(initial: &str) -> io::Result<QueueState> {
    let path = PathBuf::from(initial);
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        initial.to_string()
    };

    // Try parsing as QueueState first (resuming from state file)
    if let Ok(state) = serde_json::from_str::<QueueState>(&content) {
        return Ok(state);
    }

    // Fall back to parsing as Vec<Task> (normal initial state)
    let tasks: Vec<Task> = json5::from_str(&content).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("[E057] invalid JSON: {e}"))
    })?;

    Ok(QueueState::from_tasks(tasks))
}
```

### 4. Write state in run loop

**File:** `crates/gsd_config/src/runner.rs`

```rust
pub fn run(...) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config)?;

    while let Some(outcome) = runner.next() {
        // ... logging ...

        if let Some(path) = runner_config.state_output {
            let state = runner.state();
            std::fs::write(path, serde_json::to_vec_pretty(&state)?)?;
        }
    }

    // Delete state file on successful completion
    if let Some(path) = runner_config.state_output {
        let _ = std::fs::remove_file(path);
    }

    Ok(())
}
```

## What We Don't Track

- **In-flight tasks**: Tasks currently being processed by agents. On resume, these are lost. The agent might complete them, but we'll treat them as not started. This can cause duplicate work but is simpler than tracking in-flight state.

- **Finally hook state**: The `finally_tracking` HashMap. On resume, finally hooks won't fire correctly if we were mid-fan-out. This is a known limitation for v1.

## Files to Change

| File | Changes |
|------|---------|
| `crates/gsd_config/src/queue_state.rs` | **New file** - QueueState type |
| `crates/gsd_config/src/lib.rs` | Export queue_state module |
| `crates/gsd_config/src/runner.rs` | Use QueueState in RunnerConfig, track completed |
| `crates/gsd_cli/src/main.rs` | Add --state-output, modify parsing to return QueueState |

## Implementation Plan

1. Add `QueueState` type
2. Modify `RunnerConfig` to take `QueueState` instead of `Vec<Task>`
3. Update `TaskRunner` to track completed tasks
4. Add `--state-output` CLI flag
5. Write state on each completion, delete on success
6. Modify `parse_initial_state` to handle both formats
7. Integration test: run partially, resume, verify completion

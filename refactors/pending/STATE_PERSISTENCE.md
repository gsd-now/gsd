# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a folder with two files:
- `config.json` - the fully inlined config (everything needed to run)
- `tasks.log` - append-only stream of task outcomes

On resume, replay the log to reconstruct pending tasks. No explicit pending state needed.

## Run Folder Structure

```
<root>/runs/<pool>/<run-id>/
├── config.json    # Fully inlined config
└── tasks.log      # Newline-delimited JSON stream of outcomes
```

Example: `/tmp/agent_pool/runs/mypool/a3f2c1/`

## Task Log Format

Each line in `tasks.log` is a JSON object with internal tagging (`#[serde(tag = "kind")]`):

```json
{"kind":"Queued","value":{"id":1,"step":"Analyze","value":{...},"retries_remaining":3}}
{"kind":"Queued","value":{"id":2,"step":"Analyze","value":{...},"retries_remaining":3}}
{"kind":"Resolved","value":{"kind":"Completed","id":1,"spawned":[{"id":3,"step":"Process","value":{...},"retries_remaining":2}]}}
{"kind":"Queued","value":{"id":3,"step":"Process","value":{...},"retries_remaining":2}}
{"kind":"Resolved","value":{"kind":"Failed","id":2,"reason":{"kind":"Timeout"}}}
```

## Data Structures

```rust
// crates/gsd_config/src/run_log.rs

use serde::{Deserialize, Serialize};

/// Unique identifier for a task within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogTaskId(pub u64);

/// A log entry (one line in tasks.log).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum LogEntry {
    /// Task added to the queue.
    Queued(TaskQueueItem),
    /// Task resolved (completed or failed).
    Resolved(TaskResolution),
}

/// A task that was added to the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskQueueItem {
    pub id: LogTaskId,
    pub step: String,
    pub value: serde_json::Value,
    pub retries_remaining: u32,
}

/// A task that was resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum TaskResolution {
    /// Task completed successfully.
    Completed(TaskCompletion),
    /// Task failed.
    Failed(TaskFailure),
}

/// A successful task completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompletion {
    pub id: LogTaskId,
    /// Tasks spawned by this completion.
    pub spawned: Vec<TaskQueueItem>,
}

/// A task failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFailure {
    pub id: LogTaskId,
    pub reason: FailureReason,
}

/// Why a task failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FailureReason {
    /// Task timed out waiting for agent response.
    Timeout,
    /// Agent disconnected/crashed during task execution.
    AgentLost,
    /// Invalid response from agent (couldn't parse).
    InvalidResponse { message: String },
}
```

## Reconstructing State on Resume

```rust
fn reconstruct_pending(entries: &[LogEntry]) -> Vec<TaskQueueItem> {
    let mut pending: HashMap<LogTaskId, TaskQueueItem> = HashMap::new();

    for entry in entries {
        match entry {
            LogEntry::Queued(item) => {
                pending.insert(item.id, item.clone());
            }
            LogEntry::Resolved(resolution) => {
                let id = match resolution {
                    TaskResolution::Completed(c) => c.id,
                    TaskResolution::Failed(f) => f.id,
                };
                pending.remove(&id);
            }
        }
    }

    pending.into_values().collect()
}
```

## Implementation Phases

### Phase 1: Run Folder Creation

**Changes:**
- Add `--log <name>` CLI flag (creates `<root>/runs/<pool>/<name>/`)
- On startup with `--log`:
  - Create run folder
  - Write `config.json` (serialize the parsed config)
  - Print: `Creating run at <path>. Resume with: gsd run --log <path>`
- Run folder is deleted on successful completion

### Phase 2: Task Logging

**Changes:**
- After each task completes/fails, append `TaskOutcome` to `tasks.log`
- Use newline-delimited JSON (one object per line)
- Flush after each write for durability

### Phase 3: Resume from Log

**Changes:**
- `--log <path>` can point to existing run folder
- If `tasks.log` exists, parse and reconstruct pending tasks
- Continue from where we left off

**Detection:**
- If `<path>/config.json` exists → it's a run folder (resume)
- Otherwise → create new run folder

## CLI

```bash
# Normal run (no persistence)
gsd run config.jsonc --pool mypool --initial-state '[{"kind": "Start", "value": {}}]'

# Run with logging for resume capability
gsd run config.jsonc --pool mypool --initial-state '[...]' --log myrun
# Creates: /tmp/agent_pool/runs/mypool/myrun/
# Prints: Creating run at /tmp/agent_pool/runs/mypool/myrun/. Resume with: gsd run --log /tmp/agent_pool/runs/mypool/myrun/

# Resume from run folder (config is in the folder, no config.jsonc needed)
gsd run --log /tmp/agent_pool/runs/mypool/myrun/
```

## What We Don't Track (v1)

- **In-flight tasks**: On resume, tasks being processed are lost. May cause duplicate work.
- **Finally hook state**: On resume, finally hooks won't fire correctly if mid-fan-out.

## Future Work (TODOs)

Add to todos.md:

### List Runs

```bash
gsd runs list --root /tmp/agent_pool
# Shows:
# mypool/a3f2c1 (3 pending, 5 completed, 2 failed)
# mypool/b7d4e2 (0 pending, 12 completed, 0 failed) [complete]
```

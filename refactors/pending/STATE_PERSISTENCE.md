# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete)

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

Each line in `tasks.log` is a JSON object with external tagging (serde default):

```json
{"Completed":{"step":"Analyze","value":{...},"retries_remaining":3,"spawned":[{"kind":"Process","value":{}}]}}
{"Completed":{"step":"Process","value":{...},"retries_remaining":2,"spawned":[]}}
{"Failed":{"step":"Validate","value":{...},"retries_remaining":0,"reason":{"Timeout":null}}}
```

## Data Structures

```rust
// crates/gsd_config/src/run_log.rs

use crate::types::StepName;
use serde::{Deserialize, Serialize};

/// A task input (what gets queued).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    pub kind: StepName,
    pub value: serde_json::Value,
}

/// The outcome of a task (one line in tasks.log).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskOutcome {
    /// Task completed successfully.
    Completed {
        step: StepName,
        value: serde_json::Value,
        retries_remaining: u32,
        /// Tasks spawned by this completion.
        spawned: Vec<TaskInput>,
    },
    /// Task failed.
    Failed {
        step: StepName,
        value: serde_json::Value,
        retries_remaining: u32,
        reason: FailureReason,
    },
}

/// Why a task failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureReason {
    /// Agent returned an error response.
    Error(String),
    /// Task timed out waiting for agent response.
    Timeout,
    /// Agent disconnected/crashed during task execution.
    AgentLost,
    /// Invalid response from agent (couldn't parse).
    InvalidResponse(String),
}
```

## Reconstructing State on Resume

```rust
fn reconstruct_pending(
    initial: Vec<TaskInput>,
    outcomes: &[TaskOutcome],
) -> Vec<TaskInput> {
    // Start with initial tasks
    let mut all_queued: Vec<TaskInput> = initial;
    let mut completed_count = 0;

    // For each outcome, track what was spawned
    for outcome in outcomes {
        completed_count += 1;
        if let TaskOutcome::Completed { spawned, .. } = outcome {
            all_queued.extend(spawned.iter().cloned());
        }
    }

    // Pending = everything queued minus everything completed
    // (outcomes are in order, so skip first N)
    all_queued.into_iter().skip(completed_count).collect()
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

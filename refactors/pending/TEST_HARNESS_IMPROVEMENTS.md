# Test Harness Improvements

## Overview

This document describes improvements needed to get the agent_pool tests into a robust, comprehensive state.

---

## Test Inventory

### Current Tests

| File | Test | What It Tests | Submission Mode | Agent Mode |
|------|------|---------------|-----------------|------------|
| `greeting.rs` | `greeting_casual_and_formal` | Custom processor (greeting agent with casual/formal) | rstest × 4 | CLI |
| `single_basic.rs` | `single_agent_single_task` | Basic single agent, single task | rstest × 4 | CLI |
| `single_agent_queue.rs` | `single_agent_queues_multiple_tasks` | Single agent processes 4 tasks (queuing) | rstest × 4 | CLI |
| `many_agents.rs` | `multiple_agents_parallel_tasks` | 3 agents process 6 tasks in parallel | rstest × 4 | CLI |
| `integration.rs` | `basic_submit` | Basic submit/response flow | rstest × 4 | CLI |
| `integration.rs` | `single_agent_multiple_tasks` | Sequential tasks to single agent | rstest × 4 | CLI |
| `integration.rs` | `multiple_agents_parallel` | 2 agents process 4 tasks in parallel | rstest × 4 | CLI |
| `integration.rs` | `agent_deregistration` | Agent stops, new agent picks up work | rstest × 4 | CLI |
| `integration.rs` | `tasks_queued_before_agents` | Tasks submitted before agent registers | rstest × 4 | CLI |
| `integration.rs` | `rapid_task_burst` | 10 tasks submitted rapidly | rstest × 4 | CLI |
| `integration.rs` | `identical_task_content` | 5 tasks with identical content | rstest × 4 | CLI |
| `integration.rs` | `agent_joins_mid_processing` | Second agent joins while first is processing | rstest × 4 | CLI |
| `integration.rs` | `response_isolation` | Responses go to correct submitters | rstest × 4 | CLI |

### Submission Modes (rstest × 4)

Currently all tests use rstest with 4 submission modes:
- `DataSocket` - `--data` with `--notify socket`
- `DataFile` - `--data` with `--notify file`
- `FileSocket` - `--file` with `--notify socket`
- `FileFile` - `--file` with `--notify file`

**Missing:** Raw file protocol (direct writes to `pending/` directory)

### Agent Modes

Currently all agents use the CLI (`register`, `next_task`).

**Missing:** Raw file protocol (direct writes to `agents/` directory)

---

## Mode Matrix

The full matrix should cover:

### Submission Modes (5 total)

| Mode | CLI Flag | Description |
|------|----------|-------------|
| `DataSocket` | `--data --notify socket` | Inline JSON, socket notification |
| `DataFile` | `--data --notify file` | Inline JSON, file notification |
| `FileSocket` | `--file --notify socket` | JSON from file, socket notification |
| `FileFile` | `--file --notify file` | JSON from file, file notification |
| `RawFile` | N/A (direct write) | Write directly to `pending/<task_id>/task.json` |

### Agent Modes (2 total)

| Mode | Description |
|------|-------------|
| `CLI` | Use `register`/`next_task` CLI commands |
| `RawFile` | Write directly to `agents/<name>/response.json` |

### Coverage Goal

Run each test scenario with:
- 5 submission modes
- 2 agent modes

= 10 combinations per test

**Note:** Not all combinations make equal sense. For example, `RawFile` submission with socket notification doesn't exist. We should test the realistic combinations.

---

## Completed Work

### 1. CLI-Based TestAgent (DONE)
TestAgent uses CLI commands (`register`, `next_task`).

### 2. CLI-Based Task Submission (DONE)
All tests use `submit_with_mode()` instead of library functions.

### 3. Multi-Mode Testing with rstest (DONE)
All tests use `#[rstest]` with 4 submission modes.

### 4. CLI Rename (DONE)
`get_task` renamed to `register`.

---

## Remaining Tasks

### Task 1: Add Raw File Protocol to Submission Modes

Add a 5th submission mode that writes directly to the `pending/` directory:

```rust
pub enum SubmitMode {
    DataSocket,
    DataFile,
    FileSocket,
    FileFile,
    RawFile,  // NEW: Direct write to pending/
}
```

Implementation for `RawFile`:
```rust
SubmitMode::RawFile => {
    let task_id = uuid::Uuid::new_v4().to_string();
    let submission_dir = root.join(PENDING_DIR).join(&task_id);
    fs::create_dir_all(&submission_dir)?;

    // Write payload wrapped in Inline envelope
    let payload = serde_json::json!({
        "kind": "Inline",
        "content": payload_json
    });
    fs::write(submission_dir.join(TASK_FILE), payload.to_string())?;

    // Poll for response (or use notify)
    let response_file = submission_dir.join(RESPONSE_FILE);
    // ... wait for response ...
}
```

### Task 2: Add Raw File Protocol to Agent Modes

Add a `RawFileAgent` variant that writes directly to `agents/<name>/response.json`:

```rust
pub enum AgentMode {
    CLI,
    RawFile,
}

impl TestAgent {
    pub fn with_mode(mode: AgentMode, ...) -> Self {
        match mode {
            AgentMode::CLI => Self::start(...),
            AgentMode::RawFile => Self::start_raw_file(...),
        }
    }
}
```

### Task 3: CLI Command Improvements

#### 3.1: Consider `complete_task` for final response

Current lifecycle:
```
register -> (next_task --data <response>)* -> deregister
```

Consider adding `complete_task`:
```
register -> (next_task --data <response>)* -> complete_task --data <final_response>
```

Or add `--data` flag to `deregister`:
```
register -> (next_task --data <response>)* -> deregister --data <final_response>
```

**Open question:** What's the cleanest API?

### Task 4: Test Output Improvements

#### 4.1: Structured logging with tracing

Replace `eprintln!()` with structured tracing:
```rust
use tracing::{info, debug};
info!(agent = %agent_id, "received task");
```

#### 4.2: Tracing subscriber setup

Add test helper:
```rust
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("agent_pool=debug")
        .with_test_writer()
        .try_init();
}
```

### Task 5: Proper Teardown

Ensure tests clean up properly even on panic. Use `scopeguard` or similar.

### Task 6: Missing Test Scenarios

| Scenario | Priority | Notes |
|----------|----------|-------|
| Agent timeout | High | Agent assigned task but doesn't respond |
| Agent crash | High | Agent process dies mid-task |
| Heartbeat failure | Medium | Agent fails to respond to heartbeat |
| Task cancellation | Medium | Client withdraws task before completion |
| Large payloads | Low | Tasks with very large data |

---

## Implementation Order

1. **Task 1: Raw File Submission** - Complete the mode matrix
2. **Task 2: Raw File Agent** - Complete the mode matrix
3. **Task 5: Proper Teardown** - Reliability
4. **Task 6: Missing Scenarios** - Coverage
5. **Task 4: Test Output** - Debugging
6. **Task 3: CLI Improvements** - UX

---

## Notes

- Each test file uses its own subdirectory in `.test-data/` for parallel execution
- Tests should pass regardless of which agent is assigned a task
- `integration.rs` was converted from raw file protocol to CLI - now tests same things as other files

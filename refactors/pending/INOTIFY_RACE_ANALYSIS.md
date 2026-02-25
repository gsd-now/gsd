# Inotify Race Condition Analysis

## The Problem

Tests pass on macOS but fail (hang) on Linux. The root cause is a race condition inherent to `inotify` that doesn't exist in `FSEvents`.

### FSEvents vs Inotify

**macOS FSEvents:**
- Directory-level monitoring
- Watches entire tree automatically
- No race when subdirectories are created

**Linux inotify:**
- Per-directory watches
- Must manually add watches for new subdirectories
- **Race condition**: When a new directory is created, there's a window between:
  1. Receiving the CREATE event for the directory
  2. Adding a watch for that directory
- Files written during this window are missed

## Where The Race Occurs

### Pending Task Submission (the actual problem)

```
Submitter process:          Daemon process:
─────────────────          ──────────────────
create_dir() returns
write(task.json)  ←─────── Race: if this happens before
                           inotify_add_watch() completes,
                           we miss it

                           inotify_add_watch() completes
                           callback called
                           IoEvent::Fs(PendingDir) queued
```

The submitter doesn't wait for anything. It writes `task.json` immediately after `create_dir()` returns. This can happen before the watch is set up.

### Agents (not affected, but flattening for consistency)

Agents are immune because `response.json` can only be written after reading `task.json`, which the daemon only writes after processing `AgentDir`, which happens after the watch is set up. The causal chain guarantees ordering.

However, we'll flatten agents too for consistency and to simplify the protocol.

---

## Implementation Plan

Four phases, each independently deployable:

1. **Canary sync** - Ensure watchers are active at startup
2. **Flatten pending** - Fix the race condition (priority: unblocks CI)
3. **Flatten agents** - Consistency, reuse logic from phase 2
4. **Anonymous worker model** - Simplify agent protocol to a task queue

---

## Phase 1: Canary Sync for Both Directories at Startup

**Goal:** Ensure both `pending/` and `agents/` directories are being watched before accepting any connections or processing events.

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Changes:**

1. After creating both directories, write a canary file to each
2. Spin until we've observed FS events for both canary files
3. Panic if we see non-FS events (shouldn't happen at startup)
4. Only then proceed with startup

```rust
// Create both directories
fs::create_dir_all(&pending_dir)?;
fs::create_dir_all(&agents_dir)?;

// Sync both directories with canary files
let pending_canary = pending_dir.join(".canary");
let agents_canary = agents_dir.join(".canary");

sync_directories_with_watcher(&[&pending_canary, &agents_canary], &io_rx)?;
```

---

## Phase 2: Flatten Pending Directory

**Goal:** Eliminate the race condition for task submission by using flat files.

**Priority:** This fixes CI. Push after completing this phase.

**Current structure:**
```
pending/
├── <uuid>/
│   ├── task.json
│   └── response.json
```

**New structure:**
```
pending/
├── <id>.task.json
└── <id>.response.json
```

**Files to modify:**

1. **`constants.rs`** - Add suffixes:
   ```rust
   pub const TASK_SUFFIX: &str = ".task.json";
   pub const RESPONSE_SUFFIX: &str = ".response.json";
   ```

2. **`client/submit_file.rs`** - Change submission to write flat files:
   - No `create_dir()` call
   - Write `<id>.task.json` directly to `pending/`
   - Poll for `<id>.response.json`
   - Cleanup: delete both files

3. **`daemon/path_category.rs`** - Update categorization:
   - Remove `PendingDir` variant
   - `PendingTask` matches `<id>.task.json`
   - Add `PendingResponse` (ignored, daemon writes these)

4. **`daemon/wiring.rs`** - Update event handling:
   - Remove `PendingDir` handler
   - Update `register_pending_task` to work with flat files

5. **`daemon/io.rs`** - Update `ExternalTaskMap`:
   - Store task file path instead of directory path
   - `finish()` derives response path from task path

---

## Phase 3: Flatten Agents Directory

**Goal:** Consistency with pending directory. Reuse logic from Phase 2.

**Current structure:**
```
agents/
├── <name>/
│   ├── task.json
│   └── response.json
```

**New structure:**
```
agents/
├── <id>.task.json
└── <id>.response.json
```

Note: Use daemon-assigned IDs, not agent-provided names. Names become debug-only metadata.

**Files to modify:**

1. **`daemon/path_category.rs`** - Update categorization:
   - Remove `AgentDir` variant
   - `AgentTask` matches `<id>.task.json`
   - `AgentResponse` matches `<id>.response.json`

2. **`daemon/wiring.rs`** - Update event handling:
   - Remove `handle_agent_dir`
   - Registration happens differently (see Phase 4)

3. **`daemon/io.rs`** - Update `AgentMap`:
   - Store file paths instead of directory paths

4. **Agent-side changes** - Agents write to assigned paths instead of self-named directories

5. **Update `AGENT_PROTOCOL.md`** and `SUBMISSION_PROTOCOL.md`

---

## Phase 4: Anonymous Worker Model

**Goal:** Simplify agent protocol to a task queue. Workers are anonymous; only tasks have identity.

**Current model (problems):**
- Agents have names/directories that persist across tasks
- State machine tracking (idle, working, etc.)
- Complexity around registration, kicked state, etc.
- Agent names carry semantic meaning (uniqueness assumptions)

**New model:**
- Workers are anonymous
- Worker calls `get_task`, blocks until assigned a task
- Daemon returns task content + response file path
- Worker completes task, writes response to assigned path
- Worker calls `get_task` again (back of the queue)
- Daemon only tracks tasks, not workers

**Heartbeats:** Still useful. If a worker is waiting in the queue too long without getting a task, send a heartbeat to verify it's alive. This is about queue starvation, not task completion (task timeouts handle that).

**Names:** Keep for debugging/logging only. No semantic meaning. No uniqueness requirement. A multi-threaded agent can register multiple times with the same name.

**Key insight:** The daemon doesn't care *who* is doing the work, just *whether* the work gets done. Task timeouts handle stuck workers—if a task times out, it fails (or gets reassigned).

**Changes:**

1. Remove agent identity tracking from core state machine
2. Simplify `AgentMap` to just track pending responses by task ID
3. `get_task` / `register` returns task + response path
4. `next_task` becomes just another `get_task` call
5. Remove kicked state tracking
6. Clean up CLI commands (`register`, `next_task` → maybe just `work`?)

---

## Task Order

1. Phase 1 (canary sync) - Small, independent
2. Phase 2 (flatten pending) - **Push after this to fix CI**
3. Phase 3 (flatten agents) - Reuses Phase 2 patterns
4. Phase 4 (anonymous workers) - Larger refactor, can be separate PR

Phases 1-3 should be done together before pushing. Phase 4 can be a follow-up.

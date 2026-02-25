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

```
Agent process:              Daemon process:
─────────────────          ──────────────────
create_dir() returns
                           inotify_add_watch() completes
                           callback called
                           IoEvent::Fs(AgentDir) queued
                           process AgentDir, write task.json
poll sees task.json
read task.json
write response.json  ←──── This happens AFTER watch is set up
```

Agents are immune because `response.json` can only be written after reading `task.json`, which the daemon only writes after processing `AgentDir`, which happens after the watch is set up. The causal chain guarantees ordering.

However, we'll flatten agents too for consistency and to avoid future issues if the protocol changes.

---

## Implementation Plan

### Task 1: Canary Sync for Both Directories at Startup

**Goal:** Ensure both `pending/` and `agents/` directories are being watched before accepting any connections or processing events.

**Current state:** We create a canary in `pending/` only. We should sync both directories.

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Changes:**

1. After creating both directories, write a canary file to each
2. Spin until we've observed FS events for both canary files
3. Only then proceed with startup

```rust
// Create both directories
fs::create_dir_all(&pending_dir)?;
fs::create_dir_all(&agents_dir)?;

// Sync both directories with canary files
let pending_canary = pending_dir.join(".canary");
let agents_canary = agents_dir.join(".canary");

sync_directories_with_watcher(&[&pending_canary, &agents_canary], &io_rx)?;
```

**New sync function:**
```rust
fn sync_directories_with_watcher(
    canary_paths: &[&Path],
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<()> {
    // Write canaries to all directories
    // Spin until we've seen FS events for all of them
    // Panic if we see non-FS events (shouldn't happen at startup)
}
```

---

### Task 2: Flatten Pending Directory (Priority)

**Goal:** Eliminate the race condition for task submission by using flat files.

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
├── <uuid>.task.json
└── <uuid>.response.json
```

**Files to modify:**

1. **`constants.rs`** - Add `PENDING_TASK_SUFFIX` and `PENDING_RESPONSE_SUFFIX`

2. **`client/submit_file.rs`** - Change submission to write flat files:
   - No `create_dir()` call
   - Write `<uuid>.task.json` directly
   - Poll for `<uuid>.response.json`
   - Cleanup: delete both files

3. **`daemon/path_category.rs`** - Update categorization:
   - Remove `PendingDir` variant
   - Update `PendingTask` to match `<uuid>.task.json`
   - Add `PendingResponse` to match `<uuid>.response.json` (ignored)

4. **`daemon/wiring.rs`** - Update event handling:
   - Remove `PendingDir` handler
   - Update `register_pending_task` to work with flat files

5. **`daemon/io.rs`** - Update `ExternalTaskMap`:
   - Store task file path instead of directory path
   - `finish()` derives response path from task path

**After this task:** Push and verify CI passes.

---

### Task 3: Flatten Agents Directory

**Goal:** Consistency with pending directory. Simplify the protocol.

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
├── <name>.task.json
└── <name>.response.json
```

**Files to modify:**

1. **`constants.rs`** - Add `AGENT_TASK_SUFFIX` and `AGENT_RESPONSE_SUFFIX` (or reuse pending suffixes if identical)

2. **`daemon/path_category.rs`** - Update categorization:
   - Remove `AgentDir` variant
   - Update `AgentTask` to match `<name>.task.json`
   - Update `AgentResponse` to match `<name>.response.json`

3. **`daemon/wiring.rs`** - Update event handling:
   - Remove `handle_agent_dir`
   - Registration happens on first `AgentTask` or `AgentResponse` event

4. **`daemon/io.rs`** - Update `AgentMap`:
   - Store agent name or task file path instead of directory path

5. **Agent protocol changes** - Agents will need to:
   - Not create a directory
   - Write `<name>.response.json` directly
   - Poll for `<name>.task.json`

6. **Update `AGENT_PROTOCOL.md`** and `SUBMISSION_PROTOCOL.md`

---

## Notes

- If flattening both directories together is easier due to shared data structures (e.g., `TransportMap`, `PathCategory`), combine Tasks 2 and 3.
- Tests must pass locally after each task before proceeding to the next.
- Push after Task 2 to verify CI fix, then continue with Task 3.

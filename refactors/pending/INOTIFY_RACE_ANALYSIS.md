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

### Pending Task Submission (`NotifyMethod::Raw`)

```
1. Submitter creates `pending/<uuid>/`
2. inotify receives CREATE event
3. notify crate tries to add watch for `pending/<uuid>/`
4. Submitter writes `task.json`
5. If (4) happens before (3) completes, we miss the PendingTask event
```

## Why Blocking Sync Doesn't Work

The initial approach was to sync each new directory with a canary file:

```rust
fn sync_directory_watcher(dir: &Path, io_rx: &mpsc::Receiver<IoEvent>) -> Vec<IoEvent> {
    // Write canary, wait for event, buffer other events
    ...
}
```

**The flaw:** During the main loop, we receive `IoEvent::Socket` and `IoEvent::Effect` events. These **cannot** be buffered and replayed later:
- Socket events have live TCP streams that may timeout
- Effect events may fire timers or modify state
- Blocking the loop while syncing would break real-time responsiveness

## The Solution: Flatten the Directory Structure

Instead of creating subdirectories that need new watches, use flat files:

**Current structure (creates new directories):**
```
pending/
├── abc123/
│   ├── task.json
│   └── response.json
└── def456/
    ├── task.json
    └── response.json
```

**Proposed structure (no new directories):**
```
pending/
├── abc123.task.json
├── abc123.response.json
├── def456.task.json
└── def456.response.json
```

### Why This Works

1. The startup sync ensures `pending/` is watched before we accept connections
2. All task files are written directly into `pending/` (already watched)
3. No new directories are created → no new watches needed → no race

### Agent Directories Are Different

Agent directories (`agents/<name>/`) don't have this race because:
1. The agent creates the directory
2. The daemon sees `AgentDir` event and **doesn't do anything yet**
3. The agent writes `response.json` (or reads `task.json`)
4. By the time we need to detect `response.json`, the watch is active

The race only matters for **pending** submissions where the submitter creates a directory AND writes a file in quick succession.

---

## Implementation Plan

### Task 1: Add Panic for Non-FS Events During Startup Sync

**Goal:** Ensure startup sync only sees FS events (Socket/Effect events at startup indicate a bug).

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Current code (lines 967-969):**
```rust
Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {
    // Non-FS event or timeout, keep polling
}
```

**After:**
```rust
Ok(IoEvent::Socket(..)) | Ok(IoEvent::Effect(..)) | Ok(IoEvent::Shutdown) => {
    panic!("unexpected non-FS event during startup sync");
}
Err(mpsc::RecvTimeoutError::Timeout) => {
    // Keep polling
}
```

**Rationale:** At startup, no socket listener is accepting yet and no event loop is running, so we should never see Socket or Effect events. If we do, something is wrong.

---

### Task 2: Add PathCategory Variants for Ignored Files

**Goal:** Make path categorization explicit about which files are expected but ignored.

**File:** `crates/agent_pool/src/daemon/path_category.rs`

**Current variants:**
```rust
pub(super) enum PathCategory {
    AgentDir { name: String },
    AgentResponse { name: String },
    PendingDir { uuid: String },
    PendingTask { uuid: String },
}
```

**After:**
```rust
pub(super) enum PathCategory {
    /// Agent directory: `agents/<name>/`
    AgentDir { name: String },
    /// Agent task file: `agents/<name>/task.json` (daemon writes, agent reads)
    AgentTask { name: String },
    /// Agent response file: `agents/<name>/response.json`
    AgentResponse { name: String },
    /// Pending submission directory: `pending/<uuid>/`
    PendingDir { uuid: String },
    /// Pending task file: `pending/<uuid>/task.json`
    PendingTask { uuid: String },
    /// Pending response file: `pending/<uuid>/response.json` (daemon writes)
    PendingResponse { uuid: String },
}
```

**Update `categorize_under_agents`:**
```rust
fn categorize_under_agents(path: &Path, agents_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    let name = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::AgentDir { name }),
        2 => {
            let filename = components[1].as_os_str().to_str()?;
            match filename {
                RESPONSE_FILE => Some(PathCategory::AgentResponse { name }),
                TASK_FILE => Some(PathCategory::AgentTask { name }),
                _ => None,
            }
        }
        _ => None,
    }
}
```

**Update `categorize_under_pending`:**
```rust
fn categorize_under_pending(path: &Path, pending_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(pending_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    let uuid = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::PendingDir { uuid }),
        2 => {
            let filename = components[1].as_os_str().to_str()?;
            match filename {
                TASK_FILE => Some(PathCategory::PendingTask { uuid }),
                RESPONSE_FILE => Some(PathCategory::PendingResponse { uuid }),
                _ => None,
            }
        }
        _ => None,
    }
}
```

**Update handler in wiring.rs:**
```rust
match category {
    // ... existing cases ...
    PathCategory::AgentTask { name } => {
        // Daemon writes, agent reads - ignore our own writes
        trace!(name = %name, "AgentTask: ignoring (daemon wrote this)");
    }
    PathCategory::PendingResponse { uuid } => {
        // Daemon writes, submitter reads - ignore our own writes
        trace!(uuid = %uuid, "PendingResponse: ignoring (daemon wrote this)");
    }
}
```

---

### Task 3: Add Constants for Flat File Naming

**Goal:** Define constants for the flat file naming scheme.

**File:** `crates/agent_pool/src/constants.rs`

**Add:**
```rust
/// Suffix for pending task files (flat structure).
pub const PENDING_TASK_SUFFIX: &str = ".task.json";

/// Suffix for pending response files (flat structure).
pub const PENDING_RESPONSE_SUFFIX: &str = ".response.json";
```

---

### Task 4: Update PathCategory for Flat Pending Files

**Goal:** Update path categorization to recognize flat pending files.

**File:** `crates/agent_pool/src/daemon/path_category.rs`

After the flattening, `pending/` will contain:
- `<uuid>.task.json` - submitted by client
- `<uuid>.response.json` - written by daemon

**Remove `PendingDir` variant** (no longer used).

**Update `categorize_under_pending`:**
```rust
fn categorize_under_pending(path: &Path, pending_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(pending_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Flat files directly in pending/
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    if let Some(uuid) = filename.strip_suffix(PENDING_TASK_SUFFIX) {
        return Some(PathCategory::PendingTask { uuid: uuid.to_string() });
    }

    if let Some(uuid) = filename.strip_suffix(PENDING_RESPONSE_SUFFIX) {
        return Some(PathCategory::PendingResponse { uuid: uuid.to_string() });
    }

    None
}
```

---

### Task 5: Update submit_file.rs for Flat Structure

**Goal:** Change client submission to use flat files.

**File:** `crates/agent_pool/src/client/submit_file.rs`

**Current flow:**
1. Create `pending/<uuid>/`
2. Write `pending/<uuid>/task.json`
3. Poll for `pending/<uuid>/response.json`
4. Delete `pending/<uuid>/` directory

**New flow:**
1. Write `pending/<uuid>.task.json`
2. Poll for `pending/<uuid>.response.json`
3. Delete both files

**Changes:**

```rust
// Before
let submission_dir = pending_dir.join(&submission_id);
fs::create_dir(&submission_dir)?;
let task_path = submission_dir.join(PENDING_TASK_FILE);
let response_path = submission_dir.join(PENDING_RESPONSE_FILE);
// ... cleanup ...
let _ = fs::remove_dir_all(&submission_dir);

// After
let task_path = pending_dir.join(format!("{submission_id}{PENDING_TASK_SUFFIX}"));
let response_path = pending_dir.join(format!("{submission_id}{PENDING_RESPONSE_SUFFIX}"));
// ... cleanup ...
let _ = fs::remove_file(&task_path);
let _ = fs::remove_file(&response_path);
```

**Update `cleanup_submission`:**
```rust
pub fn cleanup_submission(root: impl AsRef<Path>, submission_id: &str) -> io::Result<()> {
    let pending_dir = root.as_ref().join(PENDING_DIR);
    let task_path = pending_dir.join(format!("{submission_id}{PENDING_TASK_SUFFIX}"));
    let response_path = pending_dir.join(format!("{submission_id}{PENDING_RESPONSE_SUFFIX}"));
    let _ = fs::remove_file(&task_path);
    let _ = fs::remove_file(&response_path);
    Ok(())
}
```

---

### Task 6: Update Daemon to Handle Flat Pending Files

**Goal:** Update daemon's pending task handling for flat files.

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Key changes:**

1. **Remove `PendingDir` handling** (no longer exists)

2. **Update `register_pending_task` signature:**

The function currently takes `submission_dir: &Path`. With flat files, we need to derive paths differently.

**Option A:** Pass task path directly and derive response path:
```rust
fn register_pending_task(
    task_path: &Path,
    pending_dir: &Path,  // To derive response path
    events_tx: &mpsc::Sender<Event>,
    ...
)
```

**Option B:** Pass uuid and pending_dir:
```rust
fn register_pending_task(
    uuid: &str,
    pending_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    ...
)
```

Option B is cleaner since `PathCategory::PendingTask` gives us the uuid.

**Handler update:**
```rust
PathCategory::PendingTask { uuid } => {
    if pending_dir.join(format!("{uuid}{PENDING_TASK_SUFFIX}")).exists() {
        register_pending_task(
            &uuid,
            pending_dir,
            events_tx,
            external_task_map,
            task_id_allocator,
            io_config,
        );
    }
}
```

---

### Task 7: Update ExternalTaskMap Storage Strategy

**Goal:** Decide what path to store for flat file submissions.

**File:** `crates/agent_pool/src/daemon/io.rs`

**Current:** `ExternalTaskMap` stores `submission_dir` path (e.g., `pending/<uuid>/`)

**Options:**

1. **Store task file path** (`pending/<uuid>.task.json`)
   - Pro: Direct lookup
   - Con: Need to derive response path

2. **Store uuid only** and derive paths when needed
   - Pro: Clean
   - Con: Need pending_dir reference everywhere

3. **Store response file path** (`pending/<uuid>.response.json`)
   - Pro: `finish()` writes to this path
   - Con: Need to derive task path for reading

**Recommendation:** Option 1 - store task file path. The `finish()` method can derive response path:

```rust
// In ExternalTaskMap::finish
Transport::Directory(path) => {
    // path is task file path, derive response path
    let response_path = path.with_extension("").with_extension("response.json");
    // Or: derive from filename
    let filename = path.file_name().unwrap().to_str().unwrap();
    let uuid = filename.strip_suffix(PENDING_TASK_SUFFIX).unwrap();
    let response_path = path.parent().unwrap().join(format!("{uuid}{PENDING_RESPONSE_SUFFIX}"));
    fs::write(response_path, response)?;
}
```

**Alternative:** Change `Transport::Directory` to `Transport::File` for flat files:
```rust
pub enum Transport {
    Directory(PathBuf),  // For agents (which still use directories)
    File(PathBuf),       // For flat pending files (stores response path)
    Socket(Stream),
}
```

This is cleaner and makes the distinction explicit.

---

### Task 8: Update Tests

**Goal:** Update all tests that reference pending directory structure.

**Files:**
- `crates/agent_pool/src/daemon/path_category.rs` - category tests
- `crates/agent_pool/src/daemon/wiring.rs` - pending task tests
- `crates/agent_pool/src/client/submit_file.rs` - submission tests

---

## Summary of Changes

| File | Change |
|------|--------|
| `wiring.rs` | Panic on non-FS events during startup sync |
| `path_category.rs` | Add `AgentTask`, `PendingResponse` variants |
| `wiring.rs` | Handle new `AgentTask`, `PendingResponse` (ignored) |
| `constants.rs` | Add `PENDING_TASK_SUFFIX`, `PENDING_RESPONSE_SUFFIX` |
| `path_category.rs` | Update `categorize_under_pending` for flat files |
| `path_category.rs` | Remove `PendingDir` variant |
| `submit_file.rs` | Use flat file structure |
| `wiring.rs` | Remove `PendingDir` handling |
| `wiring.rs` | Update `register_pending_task` for flat files |
| `io.rs` | Consider `Transport::File` variant for flat pending |
| Various | Update tests |

## Task Order

1. **Task 1**: Panic on non-FS events (small, independent)
2. **Task 2**: Add `AgentTask`, `PendingResponse` variants (small, independent)
3. **Task 3**: Add flat file constants (small, independent)
4. **Tasks 4-7**: Core flattening refactor (must be done together)
5. **Task 8**: Update tests

Tasks 1-3 can be done independently and merged before the main refactor. This reduces the scope of the "big bang" change in tasks 4-7.

# FS Watcher / File Protocol Cleanup

## Status: MOSTLY COMPLETE

The primary issue (inotify race) was fixed by flattening the submissions directory structure. See `../past/INOTIFY_RACE_ANALYSIS.md` for details.

**Remaining work:**
- Replace polling with notify in `submit_file.rs` (see `todos.md`)
- Document Linux vs macOS differences (low priority)

---

## Problem Summary (FIXED)

Tests passed on macOS (FSEvents) but failed/hung on Linux (inotify). The issue stemmed from how inotify handles recursive watching of newly-created subdirectories.

## Root Cause Analysis

**inotify race condition**: When watching a directory recursively with inotify:
1. A new subdirectory is created (e.g., `submissions/<uuid>/`)
2. inotify needs to add a watch for the new subdirectory
3. If files are written to the subdirectory before the watch is added, events are missed

**FSEvents doesn't have this problem** because it watches at the filesystem level, not per-directory.

**FIXED:** By flattening submissions to `<id>.request.json` files, we eliminated the subdirectory creation entirely.

## Original Architecture Issues (RESOLVED)

### 1. ~~Nested directories in `submissions/`~~ - FIXED

Previously each submission created `submissions/<uuid>/task.json`. Now uses flat files:
```
submissions/<uuid>.request.json
submissions/<uuid>.response.json
```

### 2. ~~Three notification methods with different reliability~~ - FIXED

`NotifyMethod::Raw` now works reliably with flat file structure.

### 3. ~~Temp file pattern~~ - FIXED

Atomic writes use temp files with rename, generating `Modify(Name)` events that watchers handle correctly.

### 4. ~~Watcher sync only proves startup readiness~~ - RESOLVED

With flat files, there's no per-submission race. The startup watcher sync is sufficient.

## Cleanup Tasks

### Task 1: Flatten submissions directory structure - **DONE**

Now uses:
```
submissions/<uuid>.request.json
submissions/<uuid>.response.json
```

### Task 2: Audit all temp file usage - **DONE**

Atomic writes use temp files in `scratch/` directory, then rename to final location. The `scratch/` directory is not watched, so temp file writes don't generate spurious events. The rename into `submissions/` generates the event that watchers handle.

### Task 3: SubmissionsDir fallback - **OBSOLETE**

No longer needed since we flattened the structure.

### Task 4: Replace polling with notify in submit_file - **TODO**

`submit_file.rs` still polls every 100ms for `response.json`. Should use file watcher instead.

#### Current Implementation (Polling)

**File:** `crates/agent_pool/src/client/submit_file.rs`

```rust
// Poll for response
let start = Instant::now();
loop {
    if response_path.exists() {
        // Read and parse response
        let response_content = fs::read_to_string(&response_path)?;

        // Clean up both files
        let _ = fs::remove_file(&request_path);
        let _ = fs::remove_file(&response_path);

        let response: Response = serde_json::from_str(&response_content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        return Ok(response);
    }

    // Check timeout
    if start.elapsed() > timeout {
        // Clean up on timeout
        let _ = fs::remove_file(&request_path);
        let _ = fs::remove_file(&response_path);

        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("file-based submit timed out after {timeout:?}"),
        ));
    }

    thread::sleep(POLL_INTERVAL);  // 100ms polling - WASTEFUL
}
```

**Problems:**
1. Polls every 100ms even when no activity
2. Adds latency (up to 100ms delay after response is written)
3. Wastes CPU cycles

#### Proposed Implementation (Notify-Based)

**Pattern:** Follow `wait_for_pool_ready` in `client/mod.rs` which already uses notify correctly.

**Step 1: Add CANARY_SUFFIX to constants.rs**

**File:** `crates/agent_pool/src/constants.rs`

```rust
/// Suffix for submission canary files (watcher verification).
pub const CANARY_SUFFIX: &str = ".canary";
```

**Step 2: Add explicit ignore in path_category.rs**

**File:** `crates/agent_pool/src/daemon/path_category.rs`

The daemon already ignores canary files (only `.request.json` is categorized), but add a comment for clarity:

```rust
fn categorize_under_submissions(
    path: &Path,
    event_kind: EventKind,
    submissions_dir: &Path,
) -> Option<PathCategory> {
    // ... existing code ...

    if let Some(id) = filename.strip_suffix(REQUEST_SUFFIX) {
        return Some(PathCategory::SubmissionRequest { id: id.to_string() });
    }

    // Other files (.response.json, .canary, .tmp) are ignored:
    // - Response files are written by daemon, no need to react
    // - Canary files are for client watcher verification
    // - Temp files are intermediate atomic write artifacts
    None
}
```

**Step 3: Add imports to submit_file.rs**

```rust
// Add to existing imports
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use crate::constants::{CANARY_SUFFIX, SCRATCH_DIR};  // Add these
```

**Step 4: Replace polling loop with watcher-based wait**

The new `submit_file_with_timeout` function:

```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = root.as_ref();

    // Wait for daemon to be ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // Canonicalize to match FSEvents paths (e.g., /var -> /private/var on macOS)
    let root = fs::canonicalize(root)?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let scratch_dir = root.join(SCRATCH_DIR);

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();

    // All files use the same ID with different suffixes
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path = submissions_dir.join(format!("{submission_id}{CANARY_SUFFIX}"));

    // Set up watcher BEFORE writing any files
    let (tx, rx) = mpsc::channel();
    let response_check = response_path.clone();
    let canary_check = canary_path.clone();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                for path in &event.paths {
                    if path == &response_check {
                        let _ = tx.send(WaitEvent::Response);
                    } else if path == &canary_check {
                        let _ = tx.send(WaitEvent::Canary);
                    }
                }
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(&submissions_dir, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    // Write BOTH request and canary files
    // - Request triggers daemon to start processing
    // - Canary verifies our watcher is working
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Atomic write: temp file in scratch/, then rename to submissions/
    let temp_path = scratch_dir.join(format!("{submission_id}.tmp"));
    fs::write(&temp_path, &content)?;
    fs::rename(&temp_path, &request_path)?;

    // Canary file for watcher verification
    fs::write(&canary_path, "sync")?;

    let start = Instant::now();
    let mut watcher_verified = false;

    // Wait for events
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            // Timeout - but check if response exists first
            if response_path.exists() {
                let _ = fs::remove_file(&canary_path);
                return read_and_cleanup_response(&request_path, &response_path);
            }
            // Clean up and error
            let _ = fs::remove_file(&request_path);
            let _ = fs::remove_file(&canary_path);
            if watcher_verified {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("file-based submit timed out after {timeout:?}"),
                ));
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "watcher verification timed out",
                ));
            }
        }

        match rx.recv_timeout(Duration::from_millis(100).min(remaining)) {
            Ok(WaitEvent::Response) => {
                // Response arrived - we're done!
                let _ = fs::remove_file(&canary_path);
                return read_and_cleanup_response(&request_path, &response_path);
            }
            Ok(WaitEvent::Canary) => {
                // Watcher is working - clean up canary and keep waiting
                watcher_verified = true;
                let _ = fs::remove_file(&canary_path);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No event yet - rewrite canary if not verified
                if !watcher_verified {
                    fs::write(&canary_path, start.elapsed().as_millis().to_string())?;
                }
                // Check if response exists (in case we missed the event)
                if response_path.exists() {
                    let _ = fs::remove_file(&canary_path);
                    return read_and_cleanup_response(&request_path, &response_path);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&request_path);
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected",
                ));
            }
        }
    }
}

/// Event types for the response watcher.
enum WaitEvent {
    Canary,
    Response,
}

/// Read response file, parse it, and clean up both request and response files.
fn read_and_cleanup_response(request_path: &Path, response_path: &Path) -> io::Result<Response> {
    let response_content = fs::read_to_string(response_path)?;

    // Clean up both files
    let _ = fs::remove_file(request_path);
    let _ = fs::remove_file(response_path);

    serde_json::from_str(&response_content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

#### Key Differences from Current Implementation

| Aspect | Before (Polling) | After (Notify) |
|--------|------------------|----------------|
| Wait mechanism | `thread::sleep(100ms)` in loop | `rx.recv_timeout()` on channel |
| Latency | 0-100ms after response written | Near-instant (event-driven) |
| CPU usage | Continuous polling | Blocks on channel |
| Watcher setup | None | Before writing request |
| Canary verification | None | Yes, ensures watcher works |
| Race handling | N/A (polling catches everything) | Check `exists()` as fallback |

#### Order of Operations (Critical)

1. **Set up watcher** on `submissions/` directory
2. **Write request AND canary files** (request triggers daemon, canary verifies watcher)
3. **Wait for events** in unified loop:
   - Response arrives → done, return it
   - Canary arrives → watcher verified, keep waiting
   - Timeout without canary → watcher broken, error
   - Timeout after canary → task timed out, error
4. **Read and cleanup** response

The key insight: watcher must be set up BEFORE writing files. Request and canary are written together so daemon can start processing immediately while we verify the watcher in parallel.

#### File Naming Convention

All files for a submission use the same UUID with different suffixes:
```
scratch/
└── <uuid>.tmp            # temp file for atomic write (not watched)

submissions/
├── <uuid>.request.json   # submitter writes (triggers daemon)
├── <uuid>.response.json  # daemon writes (result)
└── <uuid>.canary         # submitter writes (watcher verification)
```

The daemon only reacts to `.request.json` files - canary and response files are ignored by `categorize_under_submissions()`. The `scratch/` directory is not watched at all.

#### Error Handling

- **Watcher creation fails:** Return error (notify not available)
- **Canary verification fails:** Return timeout error (watcher broken)
- **Channel disconnected:** Return BrokenPipe error
- **Response timeout:** Clean up request file, return timeout error
- **JSON parse error:** Clean up files, return InvalidData error

#### Testing Considerations

The existing tests should continue to work since the API is unchanged. May want to add:
1. Test that verifies notify-based wait is faster than polling
2. Test with very fast daemon response (race condition)
3. Test with watcher failure (falls back to... error? or polling?)

#### Future Enhancement: Polling Fallback

If notify fails (e.g., too many watchers), could fall back to polling. This would require:

```rust
fn submit_file_with_timeout(...) -> io::Result<Response> {
    match try_submit_with_notify(root, payload, timeout) {
        Ok(response) => Ok(response),
        Err(e) if is_watcher_error(&e) => {
            warn!("notify failed, falling back to polling: {e}");
            submit_file_polling(root, payload, timeout)
        }
        Err(e) => Err(e),
    }
}
```

This is optional and can be added later if needed.

### Task 5: Document Linux vs macOS differences - **LOW PRIORITY**

Could be useful but not blocking anything now that the race is fixed.

### Task 6: inotify-specific handling - **NOT NEEDED**

The flat file structure works on both platforms. No special handling required.

### Task 7: Flatten agent directory - **FUTURE**

Agent structure could be flattened like submissions. See `ANONYMOUS_WORKERS.md` for the proposed three-file protocol (`<id>.ready.json`, `<id>.task.json`, `<id>.response.json`).

Low priority - agents work fine as-is since the directory exists before files are written (causal chain prevents race).

## Quick Wins - **DONE**

1. ~~Verify temp files are gone~~ - Temp files are intentional for atomic writes
2. ~~Add logging around task registration~~ - Debug logging added
3. ~~Test deduplication~~ - Working correctly with flat structure
4. ~~Increase test timeout logging~~ - Tests pass now

## Questions Answered

1. **Is the flat file structure worth the protocol change?** - YES. It fixed the race condition.
2. **Should we deprecate `NotifyMethod::Raw` on Linux?** - NO. Works fine now with flat structure.
3. **Is polling acceptable as a fallback?** - For now, yes. Long-term, replace with notify (tracked in todos.md).

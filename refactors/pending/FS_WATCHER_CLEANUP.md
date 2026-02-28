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

Atomic writes use temp files in the same directory, then rename. This is intentional - the rename generates events that watchers handle.

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

**Step 1: Add imports to submit_file.rs**

```rust
// Add to existing imports
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
```

**Step 2: Add canary verification constant**

```rust
/// Canary file prefix for verifying watcher is working.
const CANARY_PREFIX: &str = ".canary-";
```

**Step 3: Replace polling loop with watcher-based wait**

The new `submit_file_with_timeout` function:

```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = root.as_ref();
    let submissions_dir = root.join(SUBMISSIONS_DIR);

    // Wait for daemon to be ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();

    // Flat files directly in submissions directory
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path = submissions_dir.join(format!("{CANARY_PREFIX}{submission_id}"));

    // Canonicalize submissions_dir to match FSEvents paths
    let submissions_dir = fs::canonicalize(&submissions_dir)?;
    let response_path_canonical = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path_canonical = submissions_dir.join(format!("{CANARY_PREFIX}{submission_id}"));

    // Set up watcher BEFORE writing request (to not miss the response)
    let (tx, rx) = mpsc::channel();
    let response_check = response_path_canonical.clone();
    let canary_check = canary_path_canonical.clone();

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

    // Verify watcher is live via canary file
    let start = Instant::now();
    fs::write(&canary_path, "sync")?;

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(WaitEvent::Canary) => {
                let _ = fs::remove_file(&canary_path);
                break;
            }
            Ok(WaitEvent::Response) => {
                // Response arrived before canary (race but valid)
                let _ = fs::remove_file(&canary_path);
                return read_and_cleanup_response(&request_path, &response_path);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if start.elapsed() > timeout {
                    let _ = fs::remove_file(&canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "watcher sync timed out",
                    ));
                }
                // Rewrite canary to trigger another event
                fs::write(&canary_path, start.elapsed().as_millis().to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected",
                ));
            }
        }
    }

    // NOW write request file (atomic: write temp, rename)
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let temp_path = submissions_dir.join(format!(".{submission_id}.tmp"));
    fs::write(&temp_path, &content)?;
    fs::rename(&temp_path, &request_path)?;

    // Check if response already exists (daemon was very fast)
    if response_path.exists() {
        return read_and_cleanup_response(&request_path, &response_path);
    }

    // Wait for response via watcher
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        match rx.recv_timeout(remaining) {
            Ok(WaitEvent::Response) => {
                return read_and_cleanup_response(&request_path, &response_path);
            }
            Ok(WaitEvent::Canary) => {
                // Ignore stray canary events
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Final check in case we missed an event
                if response_path.exists() {
                    return read_and_cleanup_response(&request_path, &response_path);
                }
                // Clean up on timeout
                let _ = fs::remove_file(&request_path);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("file-based submit timed out after {timeout:?}"),
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&request_path);
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
| Race handling | N/A (polling catches everything) | Check `exists()` after watcher setup |

#### Order of Operations (Critical)

1. **Set up watcher** on `submissions/` directory
2. **Verify watcher** via canary file (ensures we won't miss events)
3. **Write request file** (triggers daemon)
4. **Check if response exists** (daemon might be very fast)
5. **Wait for response event** via channel
6. **Read and cleanup** response

The key insight: watcher must be set up BEFORE writing the request, otherwise we might miss the response event if the daemon is very fast.

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

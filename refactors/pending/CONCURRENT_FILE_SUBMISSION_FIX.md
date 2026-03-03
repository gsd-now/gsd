# Fix Concurrent File-Based Submissions Race Condition

## Status: DESIGN

## Problem

The `agent_joins_mid_processing::case_4` (FileReference + File notify mode) test fails intermittently. Root cause analysis reveals a race condition when multiple CLI processes submit tasks concurrently using file-based notification.

### Evidence from CI logs

- Test spawns 6 parallel submissions
- 3 succeed (create `request.json`), 3 fail (timeout waiting for `response.json`)
- Failing submissions create and delete canary files very quickly (~0.3ms)
- No `request.json` files are ever created for failing submissions
- Error message says "timed out waiting for response.json" (misleading - actual issue is earlier)

### Timeline Analysis

```
844.330ms - Failing canary #1 (3b7ac5d1) CREATE
844.676ms - Failing canary #1 DELETE (0.3ms later)
844.985ms - Failing canary #2 (ee4dc904) CREATE
845.023ms - Failing canary #2 DELETE
845.056ms - Failing canary #3 (27493cc6) CREATE
845.081ms - Failing canary #3 DELETE
848.002ms - Successful canary #1 (d6034cd3) CREATE
848.066ms - Successful submission #1 request.json MOVED_TO
```

The failing submissions' canaries are deleted immediately, but no `request.json` follows. The successful submissions happen 3ms later and work normally.

### Root Cause

The `VerifiedWatcher` has a design flaw when multiple watchers observe the same directory concurrently:

1. Each CLI process creates a `VerifiedWatcher` on the pool root
2. All watchers receive events for ALL filesystem activity
3. When `wait_for(status)` receives an event for ANOTHER submission's file:
   - Sets `canary = None` (deletes its own canary)
   - Checks if event matches status path - NO
   - Checks if `status.exists()` - should be YES but continues looping
4. High inotify event throughput may cause queue issues or event delivery delays

The canary verification mechanism proves the watcher is operational, but doesn't guarantee it will see specific future events reliably under concurrent load.

## Proposed Fix

### Option A: Skip canary verification when status file exists (Recommended)

The canary verification is designed to ensure the watcher is active before waiting for events. But if the target file already exists, we don't need the watcher at all - we can return immediately.

**Change in `wait_for`:**

```rust
pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
    // Fast path: file already exists - no watcher verification needed
    if target.exists() {
        return Ok(());
    }

    // Only verify watcher if we actually need to wait for events
    self.ensure_verified()?;  // New method that blocks until canary event received

    // ... rest of implementation
}
```

**New `ensure_verified` method:**

```rust
fn ensure_verified(&mut self) -> io::Result<()> {
    let WatcherState::Connected { rx, canary } = &mut self.state else {
        panic!("ensure_verified called on disconnected watcher");
    };

    if canary.is_none() {
        return Ok(());  // Already verified
    }

    let start = Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "watcher verification timed out"
            ));
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(_) => {
                *canary = None;
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(c) = canary {
                    c.retry()?;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.state = WatcherState::Disconnected;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected"
                ));
            }
        }
    }
}
```

**Key insight:** For `wait_for(status)`, the status file already exists (daemon writes it before accepting submissions). The fast path should always trigger, avoiding the event loop entirely.

For `wait_for(response)`, the event loop is needed, but by then the canary should already be verified from a previous wait_for call (or we can verify it explicitly).

### Option B: Unique canary directory per submission

Create a per-submission directory for the canary to avoid event interference:

```rust
// Instead of:
let canary_path = root.join(format!("{submission_id}.canary"));

// Use:
let canary_dir = root.join("canary").join(&submission_id);
fs::create_dir_all(&canary_dir)?;
let canary_path = canary_dir.join(".canary");
```

**Downside:** Adds complexity and more filesystem operations.

### Option C: Remove canary verification entirely

Trust that `notify` crate's watcher is operational immediately after `watch()` returns:

```rust
pub fn new(watch_dir: &Path) -> io::Result<Self> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(...)?;
    watcher.watch(watch_dir, RecursiveMode::Recursive)?;

    Ok(Self {
        _watcher: watcher,
        rx,
    })
}
```

**Downside:** May miss events on Linux if `watch()` returns before inotify is fully registered. This was the original problem that canary verification solved.

## Recommendation

**Option A** is the cleanest fix. The key insight is that `wait_for(status)` should almost always take the fast path because the daemon creates the status file before it's ready to accept submissions. The canary mechanism is only needed when the target file doesn't exist yet.

## Testing

After implementing the fix:

1. Run `agent_joins_mid_processing::case_4` multiple times locally
2. Submit to CI and verify the test passes
3. Consider adding a stress test that spawns 20+ concurrent submissions

## Files to Change

- `crates/agent_pool/src/fs.rs` - `VerifiedWatcher::wait_for` and new `ensure_verified`
- Possibly `crates/agent_pool/src/submit/file.rs` - if flow changes needed

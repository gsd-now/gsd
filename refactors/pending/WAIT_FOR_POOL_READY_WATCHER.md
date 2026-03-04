# Convert wait_for_pool_ready to use VerifiedWatcher

## Motivation

`wait_for_pool_ready` currently polls every 10ms:

```rust
// crates/agent_pool/src/submit/mod.rs:27-46
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    while !status_path.exists() {
        if start.elapsed() > timeout {
            return Err(...);
        }
        thread::sleep(Duration::from_millis(10));  // Polling!
    }
    Ok(())
}
```

Similarly, `wait_for_status_file` in the CLI polls every 100ms:

```rust
// crates/agent_pool_cli/src/main.rs:470-468
fn wait_for_status_file(status_file: &std::path::Path) -> bool {
    while start.elapsed() < TIMEOUT {
        if status_file.exists() {
            return true;
        }
        thread::sleep(POLL_INTERVAL);  // 100ms polling!
    }
    false
}
```

This is inconsistent with the rest of the codebase which uses `VerifiedWatcher` for file watching.

## Current Call Sites

| Location | Timeout | Used For |
|----------|---------|----------|
| `agent_pool/tests/common/mod.rs:686` | 10s | Test daemon startup |
| `gsd_cli/tests/common/mod.rs:272` | 10s | Test daemon startup |
| `gsd_config/tests/common/mod.rs:328` | 10s | Test daemon startup |
| `agent_pool_cli/src/main.rs:428` | 5s | CLI `get_task` command |

## Proposed Solution

Use `VerifiedWatcher` to watch for the status file. Rename existing method for clarity.

### New VerifiedWatcher Methods

One private implementation, two public conveniences:

```rust
// crates/agent_pool/src/verified_watcher.rs

impl VerifiedWatcher {
    /// Wait for a target file to exist (no timeout).
    pub fn wait_for_file(&mut self, target: &Path) -> io::Result<()> {
        self.wait_for_file_impl(target, None)
    }

    /// Wait for a target file to exist, with a timeout.
    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> io::Result<()> {
        self.wait_for_file_impl(target, Some(timeout))
    }

    fn wait_for_file_impl(
        &mut self,
        target: &Path,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        if target.exists() {
            return Ok(());
        }

        let deadline = timeout.map(|t| Instant::now() + t);

        loop {
            // Check timeout if set
            let wait_time = match deadline {
                Some(d) => {
                    let remaining = d.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("timed out waiting for {}", target.display()),
                        ));
                    }
                    remaining.min(Duration::from_millis(100))
                }
                None => Duration::from_millis(100),
            };

            match self.state.rx.recv_timeout(wait_time) {
                Ok(event) => {
                    if event.paths.iter().any(|p| p == target) || target.exists() {
                        return Ok(());
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    for canary in &mut self.state.remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }
    }
}
```

### Single Watcher Per CLI Invocation

**Goal:** Exactly one `VerifiedWatcher` per CLI process. Runtime panic if violated.

Create a `PoolWatcher` that watches all relevant directories upfront:

```rust
// crates/agent_pool/src/pool_watcher.rs

use std::sync::atomic::{AtomicBool, Ordering};
use std::path::Path;

/// Tracks whether a watcher has been created for this process.
/// Panics in debug mode if a second watcher is created.
static WATCHER_CREATED: AtomicBool = AtomicBool::new(false);

pub struct PoolWatcher {
    inner: VerifiedWatcher,
}

impl PoolWatcher {
    /// Create a watcher for all pool directories.
    /// Panics if called twice in the same process (debug builds).
    pub fn new(pool_root: &Path) -> io::Result<Self> {
        // Panic if already created (catch bugs in dev)
        let was_created = WATCHER_CREATED.swap(true, Ordering::SeqCst);
        debug_assert!(
            !was_created,
            "PoolWatcher created twice! Only one watcher per CLI invocation allowed."
        );

        let agents_dir = pool_root.join(AGENTS_DIR);
        let submissions_dir = pool_root.join(SUBMISSIONS_DIR);

        // Watch all directories we'll need
        let watch_dirs = [pool_root, &agents_dir, &submissions_dir];
        let inner = VerifiedWatcher::new(pool_root, &watch_dirs)?;

        Ok(Self { inner })
    }

    pub fn wait_for_file(&mut self, target: &Path) -> io::Result<()> {
        self.inner.wait_for_file(target)
    }

    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> io::Result<()> {
        self.inner.wait_for_file_with_timeout(target, timeout)
    }
}

impl Drop for PoolWatcher {
    fn drop(&mut self) {
        WATCHER_CREATED.store(false, Ordering::SeqCst);
    }
}
```

### All functions take the watcher

No more creating watchers internally - all functions require the shared watcher:

```rust
pub fn wait_for_pool_ready(
    watcher: &mut PoolWatcher,
    root: &Path,
    timeout: Duration,
) -> io::Result<()> {
    let status_path = root.join(STATUS_FILE);
    if status_path.exists() {
        return Ok(());
    }
    watcher.wait_for_file_with_timeout(&status_path, timeout)
}

pub fn wait_for_task(
    watcher: &mut PoolWatcher,
    pool_root: &Path,
    name: Option<&str>,
) -> io::Result<TaskAssignment> {
    // ... uses watcher.wait_for_file()
}

pub fn submit_file(
    watcher: &mut PoolWatcher,
    root: &Path,
    payload: &Payload,
) -> io::Result<Response> {
    // ... uses watcher.wait_for_file()
}
```

### CLI creates watcher once at startup

```rust
fn main() -> ExitCode {
    let pool_root = resolve_pool(...);

    // Create the ONE watcher for this CLI invocation
    let mut watcher = match PoolWatcher::new(&pool_root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to create watcher: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Pass to all operations
    wait_for_pool_ready(&mut watcher, &pool_root, timeout)?;
    wait_for_task(&mut watcher, &pool_root, name)?;
    // ...
}
```

### CLI Update

Replace `wait_for_status_file` with the library function:

```rust
// crates/agent_pool_cli/src/main.rs

// Before:
let status_file = root.join(STATUS_FILE);
if !wait_for_status_file(&status_file) {
    eprintln!("Daemon not ready...");
    return ExitCode::FAILURE;
}

// After:
if let Err(e) = wait_for_pool_ready(&root, Duration::from_secs(5)) {
    eprintln!("Daemon not ready: {e}");
    return ExitCode::FAILURE;
}
```

Then delete `wait_for_status_file` function entirely.

### Update Existing wait_for Callers

Rename `wait_for` → `wait_for_file` at call sites:

```rust
// worker.rs
watcher.wait_for_file(&task)?;

// submit/file.rs
watcher.wait_for_file(&response_path)?;
```

## Migration Steps

1. Add private `wait_for_file_impl` with `Option<Duration>`
2. Add public `wait_for_file` and `wait_for_file_with_timeout` that call impl
3. Create `PoolWatcher` wrapper with debug_assert for single-creation
4. Update `wait_for_pool_ready` to take `&mut PoolWatcher`
5. Update `wait_for_task` to take `&mut PoolWatcher`
6. Update `submit_file` to take `&mut PoolWatcher`
7. Update CLI to create `PoolWatcher` once at startup and pass to all functions
8. Delete `wait_for_status_file` from CLI
9. Run tests to verify (debug builds will panic if multiple watchers created)

## Testing

- Daemon startup in tests still works
- CLI `get_task` waits properly for daemon
- Timeout fires correctly if daemon never starts
- No polling in hot path (verify with tracing)

## Notes

The original comment said polling was needed because "the daemon clears and recreates the pool directory on startup, which would race with watcher setup." This is not actually a problem:

1. The pool root directory must exist before calling `wait_for_pool_ready` (caller creates it or it already exists)
2. We watch the root directory, not the status file specifically
3. The daemon creates subdirectories then writes the status file
4. Our watcher sees the status file creation event

The race condition concern was about watching a directory that gets deleted, but we watch the parent which persists.

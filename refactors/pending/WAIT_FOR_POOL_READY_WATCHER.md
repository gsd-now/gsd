# Convert wait_for_pool_ready to use VerifiedWatcher

**Status:** Implemented

## Motivation

`wait_for_pool_ready` polled every 10ms and `wait_for_status_file` in the CLI polled every 100ms. This was inconsistent with the rest of the codebase which uses `VerifiedWatcher` for file watching.

## What Was Done

### 1. Added new VerifiedWatcher methods

Renamed `wait_for` to private `wait_for_file_impl` and added two public convenience methods:

```rust
// crates/agent_pool/src/verified_watcher.rs

impl VerifiedWatcher {
    /// Wait for a specific file to appear (no timeout).
    pub fn wait_for_file(&mut self, target: &Path) -> io::Result<()> {
        self.wait_for_file_impl(target, None)
    }

    /// Wait for a specific file to appear with a timeout.
    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> io::Result<()> {
        self.wait_for_file_impl(target, Some(timeout))
    }

    fn wait_for_file_impl(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        if target.exists() {
            return Ok(());
        }

        let start = Instant::now();
        loop {
            if let Some(t) = timeout && start.elapsed() > t {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {}", target.display()),
                ));
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    for path in &event.paths {
                        if let Some(parent) = path.parent() {
                            self.remaining_canaries.retain(|c| c.dir() != parent);
                        }
                        if path == target {
                            return Ok(());
                        }
                    }
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    for canary in &mut self.remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
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

### 2. Updated call sites

```rust
// worker.rs - handles Option<Duration>
match timeout {
    Some(t) => watcher.wait_for_file_with_timeout(&task, t)?,
    None => watcher.wait_for_file(&task)?,
}

// submit/file.rs
watcher.wait_for_file_with_timeout(&status_path, POOL_READY_TIMEOUT)?;
watcher.wait_for_file_with_timeout(&response_path, timeout)?;

// submit/socket.rs
watcher.wait_for_file_with_timeout(&status_path, POOL_READY_TIMEOUT)?;
```

### 3. Deleted polling functions

- Deleted `wait_for_pool_ready` from `submit/mod.rs`
- Deleted `wait_for_status_file` from `agent_pool_cli/src/main.rs`
- Removed `wait_for_pool_ready` from lib.rs exports

### 4. Updated CLI to use watcher

```rust
// GetTask command now uses watcher directly
let status_file = root.join(STATUS_FILE);
if let Err(e) = watcher.wait_for_file_with_timeout(&status_file, Duration::from_secs(5)) {
    eprintln!("Daemon not ready: {e}");
    return ExitCode::FAILURE;
}
```

### 5. Updated test helpers

Test helpers in `agent_pool/tests/common/mod.rs`, `gsd_cli/tests/common/mod.rs`, and `gsd_config/tests/common/mod.rs` now create a watcher and use `wait_for_file_with_timeout` instead of calling `wait_for_pool_ready`.

## Files Changed

- `crates/agent_pool/src/verified_watcher.rs` - Added new methods
- `crates/agent_pool/src/worker.rs` - Updated call site
- `crates/agent_pool/src/submit/file.rs` - Updated call sites
- `crates/agent_pool/src/submit/socket.rs` - Updated call site
- `crates/agent_pool/src/submit/mod.rs` - Deleted `wait_for_pool_ready`
- `crates/agent_pool/src/lib.rs` - Removed export
- `crates/agent_pool_cli/src/main.rs` - Deleted `wait_for_status_file`, updated GetTask
- `crates/agent_pool/tests/common/mod.rs` - Updated to use watcher
- `crates/gsd_cli/tests/common/mod.rs` - Updated to use watcher
- `crates/gsd_config/tests/common/mod.rs` - Updated to use watcher

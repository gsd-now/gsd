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

Rename existing `wait_for` method and add timeout variant. Use private implementation.

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

### Update Existing wait_for Callers

Rename `wait_for` → `wait_for_file` at call sites:

```rust
// worker.rs
watcher.wait_for_file(&task)?;

// submit/file.rs
watcher.wait_for_file(&response_path)?;
```

### CLI Update

Delete `wait_for_status_file` from CLI - it will use library functions via `PoolWatcher` (see `SINGLE_WATCHER_AT_CLI_ROOT.md`).

## Migration Steps

1. Add private `wait_for_file_impl` with `Option<Duration>`
2. Add public `wait_for_file` and `wait_for_file_with_timeout` that call impl
3. Rename all `wait_for` call sites to `wait_for_file`
4. Delete `wait_for_status_file` from CLI
5. Run tests to verify

## Testing

- All existing tests pass
- Timeout fires correctly
- No polling in hot path (verify with tracing)

## Notes

The original comment said polling was needed because "the daemon clears and recreates the pool directory on startup, which would race with watcher setup." This is not actually a problem:

1. The pool root directory must exist before calling `wait_for_pool_ready` (caller creates it or it already exists)
2. We watch the root directory, not the status file specifically
3. The daemon creates subdirectories then writes the status file
4. Our watcher sees the status file creation event

The race condition concern was about watching a directory that gets deleted, but we watch the parent which persists.

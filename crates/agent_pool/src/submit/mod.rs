//! Task submission operations for interacting with the agent pool daemon.

mod file;
mod payload;
mod socket;
mod stop;

use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::constants::STATUS_FILE;

pub use file::{submit_file, submit_file_with_timeout};
pub use payload::Payload;
pub use socket::submit;
pub use stop::stop;

/// Wait for the agent pool daemon to become ready.
///
/// Polls for the status file to appear. The status file is written after the
/// daemon has completed all setup (directories created, watcher verified,
/// socket listening), so its presence indicates the daemon is fully ready.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before the pool becomes ready.
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    let root = root.as_ref();
    let status_path = root.join(STATUS_FILE);
    let start = Instant::now();

    // Poll for status file - daemon writes this after all setup is complete.
    // We poll rather than using a watcher because the daemon clears and
    // recreates the pool directory on startup, which would race with watcher setup.
    while !status_path.exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pool did not become ready: {}", root.display()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

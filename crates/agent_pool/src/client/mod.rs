//! Client operations for interacting with the agent pool daemon.

mod payload;
mod stop;
mod submit;
mod submit_file;

use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::constants::STATUS_FILE;

pub use payload::Payload;
pub use stop::stop;
pub use submit::submit;
pub use submit_file::{cleanup_submission, submit_file};

/// Default timeout for waiting for the pool to become ready.
pub const DEFAULT_POOL_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Polling interval when waiting for pool readiness.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait for the agent pool daemon to become ready.
///
/// Polls for the existence of the status file, which the daemon writes
/// once it has completed initialization and is ready to accept connections.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before the pool becomes ready.
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    let status_file = root.as_ref().join(STATUS_FILE);
    let start = Instant::now();
    while !status_file.exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "agent pool did not become ready within {:?} (status file: {})",
                    timeout,
                    status_file.display()
                ),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

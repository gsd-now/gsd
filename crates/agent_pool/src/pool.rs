//! Pool ID management.
//!
//! Pools live in `<pool_root>/<id>/` with short, memorable IDs.
//! Default pool root on Unix: `/tmp/agent_pool/`
//! Default pool root on Windows: `%TEMP%\agent_pool\`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Get the default base directory for all pools.
///
/// Uses /tmp explicitly on Unix to ensure atomic writes (which also use /tmp)
/// are on the same filesystem.
#[must_use]
pub fn default_pool_root() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp/agent_pool")
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("agent_pool")
    }
}

/// Length of generated pool IDs.
const ID_LENGTH: usize = 8;

/// Characters used for ID generation (lowercase alphanumeric, no confusing chars).
const ID_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Generate a short random pool ID.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional truncation for randomness
pub fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Simple random using time + process id as seed
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64
        ^ u64::from(std::process::id());

    let mut id = String::with_capacity(ID_LENGTH);
    let mut state = seed;

    for _ in 0..ID_LENGTH {
        // Simple xorshift for randomness
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        let idx = (state as usize) % ID_CHARS.len();
        id.push(ID_CHARS[idx] as char);
    }

    id
}

/// Get the path for a pool ID within the given pool root.
#[must_use]
pub fn id_to_path(pool_root: &Path, id: &str) -> PathBuf {
    pool_root.join(id)
}

/// Information about a pool.
#[derive(Debug)]
pub struct PoolInfo {
    /// Pool ID.
    pub id: String,
    /// Full path to the pool directory.
    pub path: PathBuf,
    /// Whether the pool is currently running.
    pub running: bool,
}

/// List all pools in the given pool root directory.
///
/// # Errors
///
/// Returns an error if the pools directory cannot be read.
pub fn list_pools(pool_root: &Path) -> io::Result<Vec<PoolInfo>> {
    if !pool_root.exists() {
        return Ok(Vec::new());
    }

    let mut pools = Vec::new();

    for entry in fs::read_dir(pool_root)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let running = is_pool_running(&path);

        pools.push(PoolInfo {
            id: id.to_string(),
            path,
            running,
        });
    }

    Ok(pools)
}

/// Check if a pool is running by verifying the lock file PID is alive.
#[cfg(unix)]
fn is_pool_running(pool_path: &std::path::Path) -> bool {
    use std::fs;

    let lock_path = pool_path.join(crate::constants::LOCK_FILE);

    let Ok(pid_str) = fs::read_to_string(&lock_path) else {
        return false;
    };

    let Ok(pid) = pid_str.trim().parse::<u32>() else {
        return false;
    };

    // Check if the process is still alive using kill -0
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check if a pool is running (Windows stub - always returns false).
#[cfg(not(unix))]
fn is_pool_running(_pool_path: &std::path::Path) -> bool {
    // On Windows, we'd need different logic to check process status.
    false
}

/// Resolve a pool reference (ID or path) to a full path.
///
/// If the input looks like a path (contains `/`), returns it as-is.
/// Otherwise, treats it as an ID and converts to `<pool_root>/<id>`.
#[must_use]
pub fn resolve_pool(pool_root: &Path, reference: &str) -> PathBuf {
    if reference.contains('/') {
        PathBuf::from(reference)
    } else {
        id_to_path(pool_root, reference)
    }
}

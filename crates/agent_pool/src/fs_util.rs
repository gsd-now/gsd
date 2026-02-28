//! Filesystem utilities for atomic operations.

use crate::constants::SCRATCH_DIR;
use std::fs;
use std::io;
use std::path::Path;
use uuid::Uuid;

/// Write content to a file atomically.
///
/// Writes to a temp file in `<pool_root>/scratch/`, then renames to the target path.
/// This ensures the target file either doesn't exist or contains complete content -
/// never a partial write.
///
/// # Arguments
///
/// * `pool_root` - The pool root directory (must contain `scratch/` subdirectory)
/// * `target` - The final path where the file should appear
/// * `content` - The content to write
///
/// # Errors
///
/// Returns an error if the write or rename fails.
pub fn atomic_write(pool_root: &Path, target: &Path, content: &[u8]) -> io::Result<()> {
    let scratch_dir = pool_root.join(SCRATCH_DIR);
    let temp_path = scratch_dir.join(format!("{}.tmp", Uuid::new_v4()));

    fs::write(&temp_path, content)?;
    fs::rename(&temp_path, target)?;

    Ok(())
}

/// Write a string to a file atomically.
///
/// Convenience wrapper around [`atomic_write`] for string content.
///
/// # Errors
///
/// Returns an error if the write or rename fails.
pub fn atomic_write_str(pool_root: &Path, target: &Path, content: &str) -> io::Result<()> {
    atomic_write(pool_root, target, content.as_bytes())
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create scratch directory
        fs::create_dir(root.join(SCRATCH_DIR)).unwrap();

        let target = root.join("test.txt");
        atomic_write_str(root, &target, "hello world").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join(SCRATCH_DIR)).unwrap();

        let target = root.join("test.txt");
        fs::write(&target, "old content").unwrap();

        atomic_write_str(root, &target, "new content").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
    }

    #[test]
    fn temp_file_cleaned_up() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let scratch = root.join(SCRATCH_DIR);
        fs::create_dir(&scratch).unwrap();

        let target = root.join("test.txt");
        atomic_write_str(root, &target, "content").unwrap();

        // Scratch directory should be empty (temp file renamed away)
        let entries: Vec<_> = fs::read_dir(&scratch).unwrap().collect();
        assert!(entries.is_empty());
    }
}

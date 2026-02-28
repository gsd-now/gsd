# Unified Watcher Verification

## Status: PENDING

Unify all file watcher usage to follow a consistent pattern with canary-based verification.

## Goals

1. **One watcher per use case** - no creating multiple watchers for a single operation
2. **Every watcher verified with canary** - confirm watcher works before relying on it
3. **Short-circuit on target file** - if we see the file we're waiting for, watcher is implicitly verified
4. **Return the watcher** - caller can continue using it for subsequent waits

## Current Problems

### Problem 1: Double watcher in submit_file

Currently `submit_file()` creates two watchers:

```rust
// First watcher: wait_for_pool_ready()
pub fn submit_file(...) {
    wait_for_pool_ready(root, timeout)?;  // Creates watcher #1, waits for status file

    // ... later in the new notify-based implementation ...
    let watcher = ...;  // Creates watcher #2, waits for response
}
```

### Problem 2: Inconsistent canary patterns

Different places do canary verification differently:
- `wait_for_pool_ready` - has canary verification
- `daemon/wiring.rs` - has canary verification (complex multi-directory)
- `agent.rs` - no canary verification currently

### Problem 3: Can't reuse watchers

Each function creates and discards its own watcher. No way to reuse a verified watcher for multiple waits.

---

## Design

### Key Insight

A canary is just another file we're waiting for. The only difference is:
- **Canary**: We write it, retry if not seen, clean up after
- **Target**: Something else writes it, we just wait

So the API should accept a **set of items to wait for**, where each item is either a canary or a target.

### Core Abstraction

```rust
/// What to wait for.
pub enum WaitItem {
    /// A canary file - watcher creates it, retries writes, cleans up when seen.
    Canary(PathBuf),
    /// A target file - just wait for an event on it.
    Target(PathBuf),
}

/// A file watcher verified via canary files.
pub struct VerifiedWatcher {
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
}

impl VerifiedWatcher {
    /// Create a watcher and wait for ALL items.
    ///
    /// For canaries: writes the file, retries periodically, cleans up when seen.
    /// For targets: just waits for the event.
    ///
    /// Returns when ALL items have been observed.
    ///
    /// Use this for daemon startup (verify both directories) or when you need
    /// to ensure multiple files exist before proceeding.
    pub fn wait_all(
        watch_dir: &Path,
        items: Vec<WaitItem>,
        timeout: Option<Duration>,
    ) -> io::Result<Self>;

    /// Create a watcher and wait for ANY item.
    ///
    /// Returns `(watcher, observed_path)` - which path was seen first.
    ///
    /// Use this for short-circuiting: "canary OR status file, whichever first".
    /// If the target wins, you're done. If the canary wins, use `wait_for()`
    /// to continue waiting for the target.
    pub fn wait_any(
        watch_dir: &Path,
        items: Vec<WaitItem>,
        timeout: Option<Duration>,
    ) -> io::Result<(Self, PathBuf)>;

    /// Wait for a specific file on an existing watcher.
    ///
    /// Use this after `wait_any()` when the canary was seen first and you
    /// still need to wait for the target.
    pub fn wait_for(&self, target: &Path, timeout: Option<Duration>) -> io::Result<()>;

    /// Consume the watcher and return the raw event receiver.
    ///
    /// Use this when you need custom event processing (e.g., daemon main loop).
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf>;
}
```

### Canary Retry Logic

When waiting for a canary:
1. Write file with content `"sync"`
2. If not seen within 100ms, rewrite with timestamp (triggers new event)
3. Repeat until seen or timeout
4. Delete file when seen

This handles the race where the watcher isn't fully set up when we first write.

---

## Use Cases

### Overview

| Use Case | Flow | Caller | Description |
|----------|------|--------|-------------|
| `submit_file` | **Client → Daemon** | CLI/SDK submitting tasks | Wait for pool ready, then response |
| `wait_for_pool_ready` | **Client → Daemon** | CLI/SDK checking daemon | Wait for status file (daemon alive) |
| Daemon startup | **Daemon init** | Daemon process | Verify watchers before accepting work |
| Agent task wait | **Agent → Daemon** | Agent processes | Wait for task assignment |

---

### 1. File-Based Submission (`submit_file`)

**Flow: Client → Daemon** (submitter waiting for task completion)

**Current flow (two watchers):**
```rust
pub fn submit_file_with_timeout(...) -> io::Result<Response> {
    // WATCHER #1: wait for pool ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // ... write request ...

    // WATCHER #2: wait for response (in new implementation)
    let watcher = create_watcher(...)?;
    // ... wait for response ...
}
```

**New flow (one watcher):**
```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Option<Duration>,  // None = wait forever (GSD use case)
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let submission_id = Uuid::new_v4().to_string();

    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path = submissions_dir.join(format!("{submission_id}.canary"));
    let status_path = root.join(STATUS_FILE);

    // Wait for canary OR status file (whichever first)
    let (watcher, seen) = VerifiedWatcher::wait_any(
        &submissions_dir,
        vec![
            WaitItem::Canary(canary_path),
            WaitItem::Target(status_path.clone()),
        ],
        Some(Duration::from_secs(5)),
    )?;

    // If canary was seen first, still need to wait for status
    if seen != status_path {
        watcher.wait_for(&status_path, Some(Duration::from_secs(10)))?;
    }

    // Now we know: watcher works AND pool is ready
    // Write request file
    atomic_write_str(&root, &request_path, &serde_json::to_string(payload)?)?;

    // Wait for response (None = wait forever, like GSD does)
    watcher.wait_for(&response_path, timeout)?;

    // Read and cleanup
    read_and_cleanup_response(&request_path, &response_path)
}
```

**Key points:**
- One watcher created, used for both status file and response file
- `wait_any` with canary + status file: if status wins, we're ready immediately
- Watcher is already verified when we write the request

### 2. Wait for Pool Ready (`wait_for_pool_ready`)

**Flow: Client → Daemon** (verifying daemon is alive before submission)

**Current implementation:**
```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    // ... 120 lines of watcher setup, canary verification, status file wait ...
}
```

**New implementation:**
```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Option<Duration>) -> io::Result<()> {
    let root = fs::canonicalize(root.as_ref())?;
    let status_path = root.join(STATUS_FILE);
    let canary_path = root.join("client.canary");

    // Wait for canary OR status file
    let (watcher, seen) = VerifiedWatcher::wait_any(
        &root,
        vec![
            WaitItem::Canary(canary_path),
            WaitItem::Target(status_path.clone()),
        ],
        timeout,
    )?;

    if seen == status_path {
        // Status file seen first - pool is ready
        return Ok(());
    }

    // Canary seen first - watcher verified, now wait for status
    watcher.wait_for(&status_path, timeout)
}
```

**Key points:**
- Much simpler: ~15 lines instead of ~120
- If status file already exists and we see its event, return immediately
- Otherwise canary verifies watcher, then wait for status

### 3. Daemon Startup (`daemon/wiring.rs`)

**Flow: Daemon init** (verifying filesystem watchers work before accepting connections)

**Current implementation:**
```rust
// In sync_and_setup():
// - Creates watcher on pool root
// - Creates agents/, submissions/, canary
// - Waits to see events for all created items
// - Complex logic to track which events we've seen
```

**New implementation:**

The daemon needs to verify watchers on TWO directories:
- `agents/` - for agent registration events
- `submissions/` - for file-based submission events

```rust
fn verify_daemon_watchers(
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
) -> io::Result<VerifiedWatcher> {
    // Create both directories first
    fs::create_dir_all(agents_dir)?;
    fs::create_dir_all(submissions_dir)?;

    // Wait for BOTH canaries (verifies both directories are watched)
    let watcher = VerifiedWatcher::wait_all(
        root,
        vec![
            WaitItem::Canary(agents_dir.join("daemon.canary")),
            WaitItem::Canary(submissions_dir.join("daemon.canary")),
        ],
        Some(Duration::from_secs(30)),
    )?;

    Ok(watcher)
}
```

**Key points:**
- `wait_all` with two canaries verifies both directories work
- Canary lifecycle (write, retry, cleanup) handled by `VerifiedWatcher`
- Returns watcher for use in main daemon loop

### 4. Agent Waiting for Task (`agent.rs`)

**Flow: Agent → Daemon** (registered agent waiting for work assignment)

**Current implementation:**
```rust
pub fn wait_for_task(agent_dir: &Path, timeout: Duration) -> io::Result<String> {
    // ... polling loop with thread::sleep ...
}
```

**New implementation:**
```rust
pub fn wait_for_task(agent_dir: &Path, timeout: Option<Duration>) -> io::Result<String> {
    let task_path = agent_dir.join(TASK_FILE);
    let canary_path = agent_dir.join("agent.canary");

    // Wait for canary OR task file
    let (watcher, seen) = VerifiedWatcher::wait_any(
        agent_dir,
        vec![
            WaitItem::Canary(canary_path),
            WaitItem::Target(task_path.clone()),
        ],
        Some(Duration::from_secs(5)),  // Verification timeout
    )?;

    if seen == task_path {
        // Task arrived during verification
        return fs::read_to_string(&task_path);
    }

    // Canary seen first - wait for task (None = wait forever for long-running agents)
    watcher.wait_for(&task_path, timeout)?;
    fs::read_to_string(&task_path)
}
```

**Key points:**
- If task.json arrives during canary verification, return immediately
- Same `wait_any` pattern as other use cases

---

## Implementation Plan

### Step 1: Create `VerifiedWatcher` in `fs_util.rs`

Add to existing `fs_util.rs`:

```rust
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What to wait for when creating a VerifiedWatcher.
pub enum WaitItem {
    /// A canary file - watcher writes it, retries periodically, cleans up when seen.
    Canary(PathBuf),
    /// A target file - just wait for an event on it.
    Target(PathBuf),
}

impl WaitItem {
    fn path(&self) -> &Path {
        match self {
            WaitItem::Canary(p) | WaitItem::Target(p) => p,
        }
    }

    fn is_canary(&self) -> bool {
        matches!(self, WaitItem::Canary(_))
    }
}

/// A file watcher verified via canary files.
pub struct VerifiedWatcher {
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
}

impl VerifiedWatcher {
    /// Create a watcher and wait for ALL items.
    ///
    /// For canaries: writes the file, retries periodically, cleans up when seen.
    /// For targets: just waits for the event.
    ///
    /// Returns when ALL items have been observed.
    pub fn wait_all(
        watch_dir: &Path,
        items: Vec<WaitItem>,
        timeout: Option<Duration>,
    ) -> io::Result<Self> {
        let (watcher, rx) = Self::create_watcher(watch_dir)?;
        let paths: HashSet<PathBuf> = items.iter().map(|i| i.path().to_path_buf()).collect();

        Self::wait_for_items(&rx, &items, paths, WaitMode::All, timeout)?;

        Ok(Self { rx, _watcher: watcher })
    }

    /// Create a watcher and wait for ANY item.
    ///
    /// Returns `(watcher, observed_path)` - which path was seen first.
    pub fn wait_any(
        watch_dir: &Path,
        items: Vec<WaitItem>,
        timeout: Option<Duration>,
    ) -> io::Result<(Self, PathBuf)> {
        let (watcher, rx) = Self::create_watcher(watch_dir)?;
        let paths: HashSet<PathBuf> = items.iter().map(|i| i.path().to_path_buf()).collect();

        let seen = Self::wait_for_items(&rx, &items, paths, WaitMode::Any, timeout)?;

        Ok((Self { rx, _watcher: watcher }, seen.into_iter().next().unwrap()))
    }

    /// Wait for a specific file on an existing watcher.
    pub fn wait_for(&self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        if target.exists() {
            return Ok(());
        }

        let start = Instant::now();
        loop {
            if let Some(t) = timeout {
                if start.elapsed() > t {
                    if target.exists() { return Ok(()); }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", target.display()),
                    ));
                }
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) if path == target => return Ok(()),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {
                    if target.exists() { return Ok(()); }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "watcher disconnected"));
                }
            }
        }
    }

    /// Consume the watcher and return the raw event receiver.
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf> {
        self.rx
    }

    // --- Private helpers ---

    fn create_watcher(watch_dir: &Path) -> io::Result<(RecommendedWatcher, mpsc::Receiver<PathBuf>)> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            },
            Config::default(),
        )
        .map_err(io::Error::other)?;

        watcher.watch(watch_dir, RecursiveMode::NonRecursive).map_err(io::Error::other)?;

        Ok((watcher, rx))
    }

    fn wait_for_items(
        rx: &mpsc::Receiver<PathBuf>,
        items: &[WaitItem],
        mut remaining: HashSet<PathBuf>,
        mode: WaitMode,
        timeout: Option<Duration>,
    ) -> io::Result<HashSet<PathBuf>> {
        let start = Instant::now();
        let mut seen = HashSet::new();

        // Write all canaries initially
        for item in items {
            if let WaitItem::Canary(path) = item {
                fs::write(path, "sync")?;
            }
        }

        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) => {
                    if remaining.remove(&path) {
                        seen.insert(path.clone());

                        // Clean up if it was a canary
                        if items.iter().any(|i| i.is_canary() && i.path() == path) {
                            let _ = fs::remove_file(&path);
                        }

                        // Check completion
                        match mode {
                            WaitMode::Any => {
                                // Clean up remaining canaries
                                Self::cleanup_canaries(items, &remaining);
                                return Ok(seen);
                            }
                            WaitMode::All if remaining.is_empty() => {
                                return Ok(seen);
                            }
                            WaitMode::All => {}
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Check timeout
                    if let Some(t) = timeout {
                        if start.elapsed() > t {
                            Self::cleanup_canaries(items, &remaining);
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "watcher verification timed out",
                            ));
                        }
                    }

                    // Retry canaries that haven't been seen
                    for item in items {
                        if let WaitItem::Canary(path) = item {
                            if remaining.contains(path) {
                                fs::write(path, start.elapsed().as_millis().to_string())?;
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Self::cleanup_canaries(items, &remaining);
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "watcher disconnected"));
                }
            }
        }
    }

    fn cleanup_canaries(items: &[WaitItem], remaining: &HashSet<PathBuf>) {
        for item in items {
            if let WaitItem::Canary(path) = item {
                if remaining.contains(path) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

enum WaitMode {
    All,
    Any,
}
```

### Step 2: Update `submit_file.rs`

Replace the polling implementation with `VerifiedWatcher`.

### Step 3: Update `wait_for_pool_ready` in `client/mod.rs`

Simplify to use `VerifiedWatcher`.

### Step 4: Update daemon watcher verification

Modify `sync_and_setup` to use dual-canary verification.

### Step 5: Update agent task waiting

Replace polling in `agent.rs` with `VerifiedWatcher`.

---

## File Changes Summary

| File | Change |
|------|--------|
| `crates/agent_pool/src/fs_util.rs` | Add `VerifiedWatcher` struct |
| `crates/agent_pool/src/lib.rs` | Export `VerifiedWatcher` |
| `crates/agent_pool/src/client/submit_file.rs` | Use `VerifiedWatcher`, single watcher |
| `crates/agent_pool/src/client/mod.rs` | Simplify `wait_for_pool_ready` |
| `crates/agent_pool/src/daemon/wiring.rs` | Dual-canary verification |
| `crates/agent_pool/src/agent.rs` | Use `VerifiedWatcher` for task waiting |

---

## Testing Considerations

1. **Unit tests for `VerifiedWatcher`**
   - Canary verification works
   - Short-circuit on target file works
   - Timeout handling works
   - `wait_for` correctly waits for files

2. **Integration tests unchanged**
   - Tests use CLI, which uses these functions internally
   - Behavior should be identical, just faster

3. **Manual testing**
   - Verify latency improvement (no 100ms polling delay)
   - Verify works on both macOS (FSEvents) and Linux (inotify)

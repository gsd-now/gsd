# Multi-Directory Canary Verification

## Status: PENDING APPROVAL

## Problem

On Linux with inotify, recursive file watching has a race condition. When `notify` watches a directory recursively and a new subdirectory is created, `notify` dynamically adds a watch for that subdirectory. There's a window where files can be written to the subdirectory before the watch is set up, causing events to be missed.

### Evidence

CI test `greeting_casual_and_formal::case_1` fails intermittently:
1. Daemon starts, watches pool root recursively
2. Daemon creates `agents/` directory
3. Agent writes `.ready.json` to `agents/`
4. Daemon never sees the `.ready.json` event
5. Task submission times out waiting for an agent

The daemon's canary file is written to the pool root, which proves the root watch works, but says nothing about whether the `agents/` subdirectory watch is ready.

### Flawed Assumption

Both `VerifiedWatcher` and the daemon's `sync_and_setup` have this comment:

```rust
// crates/agent_pool/src/fs.rs:136-143
/// Once we observe any filesystem event, the watcher is fully operational.
/// Filesystem watchers (`FSEvents` on macOS, `inotify` on Linux) don't "partially work".
```

This is **incorrect** for recursive watching on Linux. The root watch may work while subdirectory watches are still being set up.

## Current State

### `CanaryGuard` (fs.rs:96-117)

Manages a single canary file:

```rust
struct CanaryGuard {
    path: PathBuf,
    writes: u32,
}

impl CanaryGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        fs::write(&path, "0")?;
        Ok(Self { path, writes: 0 })
    }

    fn retry(&mut self) -> io::Result<()> {
        self.writes += 1;
        fs::write(&self.path, self.writes.to_string())
    }
}

impl Drop for CanaryGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
```

**No changes needed.** This correctly manages a single canary file.

### `WatcherState` (fs.rs:119-130)

```rust
enum WatcherState {
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,  // Single canary
    },
    Disconnected,
}
```

### `VerifiedWatcher::new` (fs.rs:160-188)

```rust
pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self> {
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

    watcher
        .watch(watch_dir, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    let canary = CanaryGuard::new(canary_path)?;  // Single canary

    Ok(Self {
        _watcher: watcher,
        state: WatcherState::Connected {
            rx,
            canary: Some(canary),
        },
    })
}
```

### `VerifiedWatcher::wait_for` verification logic (fs.rs:245-248)

```rust
// Any event proves watcher works - clean up canary
if canary.is_some() {
    *canary = None;
}
```

### Callers of `VerifiedWatcher::new`

| File | Line | Current Usage |
|------|------|---------------|
| `worker.rs` | 63 | `VerifiedWatcher::new(&agents_dir, canary)` |
| `submit/file.rs` | 86 | `VerifiedWatcher::new(&root, canary_path)` |
| `submit/mod.rs` | 54 | `VerifiedWatcher::new(&root, canary_path)` |
| `submit/socket.rs` | 35 | `VerifiedWatcher::new(&root, canary_path)` |

All callers pass a single directory. They don't need multi-directory verification.

### Daemon's `sync_and_setup` (wiring.rs:802-867)

Has its own inline canary logic, doesn't use `VerifiedWatcher`:

```rust
fn sync_and_setup(
    root: &Path,
    lock_path: &Path,
    socket_path: &Path,
    submissions_dir: &Path,
    agents_dir: &Path,
    scratch_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<(LockGuard, Listener)> {
    let canary_path = root.join("daemon.canary");  // Only root!

    // Create directories
    fs::create_dir_all(submissions_dir)?;
    fs::create_dir_all(agents_dir)?;
    fs::create_dir_all(scratch_dir)?;

    // Write canary file to trigger an event
    fs::write(&canary_path, "0")?;

    // ... wait for ANY event, then return ...
}
```

**This is the bug.** Only writes canary to root, not to subdirectories.

## Proposed Changes

### 1. Update `WatcherState` to support multiple canaries

```rust
// Before
enum WatcherState {
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,
    },
    Disconnected,
}

// After
enum WatcherState {
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canaries: Vec<CanaryGuard>,
        verified_dirs: HashSet<PathBuf>,
    },
    Disconnected,
}
```

### 2. Update `VerifiedWatcher::new` signature

```rust
// Before
pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self>

// After
pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self>
```

Implementation:

```rust
pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self> {
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

    watcher
        .watch(watch_dir, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    // Create a canary in each directory
    let canaries = canary_dirs
        .iter()
        .map(|dir| {
            let canary_path = dir.join(format!("{}.canary", Uuid::new_v4()));
            CanaryGuard::new(canary_path)
        })
        .collect::<io::Result<Vec<_>>>()?;

    Ok(Self {
        _watcher: watcher,
        state: WatcherState::Connected {
            rx,
            canaries,
            verified_dirs: HashSet::new(),
        },
    })
}
```

### 3. Update `VerifiedWatcher::wait_for` verification logic

```rust
// Before (fs.rs:245-248)
// Any event proves watcher works - clean up canary
if canary.is_some() {
    *canary = None;
}

// After
// Check which directory this event came from
if let Some(parent) = path.parent() {
    for canary in &*canaries {
        if let Some(canary_parent) = canary.path.parent() {
            if parent == canary_parent {
                verified_dirs.insert(canary_parent.to_path_buf());
            }
        }
    }
}

// Only clean up canaries when ALL directories verified
if verified_dirs.len() == canaries.len() {
    canaries.clear();
}
```

Note: `CanaryGuard` needs to expose `path` or provide a `parent_dir()` method.

### 4. Update `CanaryGuard` to expose directory

```rust
// Before
struct CanaryGuard {
    path: PathBuf,
    writes: u32,
}

// After
struct CanaryGuard {
    path: PathBuf,
    dir: PathBuf,  // The directory this canary verifies
    writes: u32,
}

impl CanaryGuard {
    fn new(dir: PathBuf) -> io::Result<Self> {
        let path = dir.join(format!("{}.canary", Uuid::new_v4()));
        fs::write(&path, "0")?;
        Ok(Self { path, dir, writes: 0 })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }

    // ... rest unchanged ...
}
```

### 5. Update callers of `VerifiedWatcher::new`

**worker.rs:63**
```rust
// Before
let mut watcher = VerifiedWatcher::new(&agents_dir, canary)?;

// After
let mut watcher = VerifiedWatcher::new(&agents_dir, &[agents_dir.clone()])?;
```

**submit/file.rs:86**
```rust
// Before
let mut watcher = VerifiedWatcher::new(&root, canary_path)?;

// After
let mut watcher = VerifiedWatcher::new(&root, &[root.clone()])?;
```

**submit/mod.rs:54**
```rust
// Before
let mut watcher = VerifiedWatcher::new(&root, canary_path)?;

// After
let mut watcher = VerifiedWatcher::new(&root, &[root.clone()])?;
```

**submit/socket.rs:35**
```rust
// Before
let mut watcher = VerifiedWatcher::new(&root, canary_path)?;

// After
let mut watcher = VerifiedWatcher::new(&root, &[root.clone()])?;
```

### 6. Update daemon's `sync_and_setup`

Two options:

**Option A: Use `VerifiedWatcher`**

Replace the inline canary logic with `VerifiedWatcher`. However, this is tricky because `sync_and_setup` receives events via `io_rx` (the unified channel), not a separate watcher.

**Option B: Update inline logic to match**

```rust
// Before (wiring.rs:814-828)
let canary_path = root.join("daemon.canary");

// Create directories
fs::create_dir_all(submissions_dir)?;
fs::create_dir_all(agents_dir)?;
fs::create_dir_all(scratch_dir)?;

// Write canary file to trigger an event
fs::write(&canary_path, "0")?;

// After
// Create directories first
fs::create_dir_all(submissions_dir)?;
fs::create_dir_all(agents_dir)?;
fs::create_dir_all(scratch_dir)?;

// Write canary files to ALL directories we need to watch
let canary_paths = [
    root.join("daemon.canary"),
    agents_dir.join("daemon.canary"),
    submissions_dir.join("daemon.canary"),
];
let mut verified_dirs: HashSet<PathBuf> = HashSet::new();

for path in &canary_paths {
    fs::write(path, "0")?;
}
```

Then update the event loop:

```rust
// Before (wiring.rs:834-843)
Ok(IoEvent::Fs(event)) => {
    // Any filesystem event proves the watcher is working.
    debug!(
        "watcher sync complete - received event {:?} for {:?}",
        event.kind, event.paths
    );
    let _ = fs::remove_file(&canary_path);
    return Ok((lock, listener));
}

// After
Ok(IoEvent::Fs(event)) => {
    // Track which directories we've seen events from
    for path in &event.paths {
        if let Some(parent) = path.parent() {
            verified_dirs.insert(parent.to_path_buf());
        }
    }

    // Only proceed when ALL directories are verified
    if verified_dirs.contains(root)
        && verified_dirs.contains(agents_dir)
        && verified_dirs.contains(submissions_dir)
    {
        debug!(
            "watcher sync complete - verified all {} directories",
            canary_paths.len()
        );
        for path in &canary_paths {
            let _ = fs::remove_file(path);
        }
        return Ok((lock, listener));
    }
}
```

I recommend **Option B** because:
- The daemon already has its own event channel (`io_rx`)
- Introducing `VerifiedWatcher` would require architectural changes to how the daemon receives events
- The fix is localized to `sync_and_setup`

## Open Questions

1. **Should we extract a shared helper?** The daemon's inline logic and `VerifiedWatcher` will have similar multi-directory verification. Should we extract a `MultiDirVerifier` trait or helper function?

2. **Retry behavior:** Currently `CanaryGuard::retry()` rewrites the canary on timeout. With multiple canaries, should we rewrite all of them or only the ones for unverified directories?

3. **Timeout semantics:** The current 5-second timeout is for seeing ANY event. With multiple directories, should it be 5 seconds total or 5 seconds per directory?

## Testing

The fix should make `greeting_casual_and_formal::case_1` (and similar tests) reliable. To verify:

1. Run `cargo test -p agent_pool --test greeting -- --test-threads=1` multiple times
2. All cases should pass consistently
3. The daemon logs should show "verified all 3 directories" instead of "received event"

# Notify-Based Agent Waiting

## Goal

Replace sleep-based polling with `notify`-based file watching for agents waiting for tasks. Test agents should use the same code path as real agents (via CLI or library).

## Current Architecture

### The Agent Protocol (File-Based)

Agents communicate with the daemon through files in `<pool>/agents/<agent_name>/`:
- `task.json` - Written by daemon when assigning work
- `response.json` - Written by agent when work is complete

**State Machine:**

| task.json | response.json | Meaning |
|-----------|---------------|---------|
| absent | absent | Idle - waiting for task |
| present | absent | Task pending - agent should process |
| present | present | Agent done - daemon should cleanup |
| absent | present | Cleanup in progress - transitionary, do nothing |

The daemon deletes task.json first, then response.json. This means `(absent, present)` is a valid transitionary state that agents will briefly observe during cleanup. Agents should simply wait - they only act when `task.exists() && !response.exists()`.

### Existing Daemon Abstractions

**Transport (io.rs:60-120):**
```rust
pub(super) enum Transport {
    Directory(PathBuf),
    Socket(Stream),
}

impl Transport {
    pub fn read(&self, filename: &str) -> io::Result<String>;
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()>;
    pub fn path(&self) -> Option<&Path>;
}
```

**Event loop (wiring.rs) - SEPARATE from Transport:**
```rust
// Notify watcher sends events to channel
let (watcher, fs_events) = create_fs_watcher(&root, wake_tx.clone())?;

// Event loop blocks on channel, processes events
loop {
    match io_rx.recv() {
        Ok(IoEvent::Fs(event)) => handle_fs_event(...),
        // ...
    }
}
```

Key insight: **Transport is just read/write. The event loop is separate.** The daemon doesn't have `Transport.wait_for()` - it has an independent event loop that watches files and calls `Transport.read()` when appropriate.

### Current Polling Implementations (Problem)

**CLI (`crates/agent_pool_cli/src/main.rs:195-239`):**
```rust
fn wait_for_task(...) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

**Test agents (`crates/agent_pool/tests/common/mod.rs`):**
```rust
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

## Target Architecture

### Key Insight: Reuse Transport + Separate Event Loop

The daemon pattern is:
1. `Transport` - just read/write operations
2. `notify` watcher - watches files, sends events to channel
3. Event loop - blocks on channel, calls Transport.read() when files change

The agent side should use the **same pattern**:
1. Reuse `Transport` as-is (move to shared module)
2. Create agent-side notify watcher
3. Agent event loop - blocks on channel, calls Transport.read() when task.json appears

### Architecture Diagram

```
DAEMON SIDE                          AGENT SIDE
============                         ===========

┌─────────────────┐                  ┌─────────────────┐
│  notify watcher │                  │  notify watcher │
│  (watches pool) │                  │  (watches agent │
└────────┬────────┘                  │   directory)    │
         │                           └────────┬────────┘
         │ fs events                          │ fs events
         ▼                                    ▼
┌─────────────────┐                  ┌─────────────────┐
│   Event Loop    │                  │   Event Loop    │
│  (io_loop in    │                  │  (agent_loop)   │
│   wiring.rs)    │                  │                 │
└────────┬────────┘                  └────────┬────────┘
         │                                    │
         │ reads/writes                       │ reads/writes
         ▼                                    ▼
┌─────────────────┐                  ┌─────────────────┐
│    Transport    │◄────────────────►│    Transport    │
│  (read/write)   │   SHARED CODE    │  (read/write)   │
└─────────────────┘                  └─────────────────┘
```

## Concrete Tasks

### Task 1: Move Transport to Shared Module

**Goal:** Make `Transport` accessible to both daemon and agent code.

**File:** `crates/agent_pool/src/daemon/io.rs` → `crates/agent_pool/src/transport.rs`

**Steps:**

1.1. Create new file `crates/agent_pool/src/transport.rs`:
```rust
//! Transport abstraction for file-based and socket-based communication.

use interprocess::local_socket::Stream;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Transport for agent/daemon communication.
pub enum Transport {
    /// File-based transport using a directory.
    Directory(PathBuf),
    /// Socket-based transport (for inline responses).
    Socket(Stream),
}

impl Transport {
    /// Read content from a file in this transport.
    pub fn read(&self, filename: &str) -> io::Result<String> {
        match self {
            Self::Directory(dir) => fs::read_to_string(dir.join(filename)),
            Self::Socket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot read from socket transport",
            )),
        }
    }

    /// Write content to a file atomically (temp file + rename).
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()> {
        match self {
            Self::Directory(dir) => {
                let target = dir.join(filename);
                let temp_name = format!(".{}.{}.tmp", filename, Uuid::new_v4());
                let temp_path = dir.join(&temp_name);

                let mut file = File::create(&temp_path)?;
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                drop(file);

                fs::rename(&temp_path, &target)?;
                Ok(())
            }
            Self::Socket(stream) => {
                use std::io::Write;
                let mut stream = stream;
                write!(stream, "{}\n{}", content.len(), content)?;
                stream.flush()
            }
        }
    }

    /// Get the directory path (only for Directory transport).
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Directory(dir) => Some(dir),
            Self::Socket(_) => None,
        }
    }
}
```

1.2. Update `crates/agent_pool/src/lib.rs`:
```rust
mod transport;
pub use transport::Transport;
```

1.3. Update `crates/agent_pool/src/daemon/io.rs` to use `crate::Transport` instead of local definition.

### Task 2: Create Agent Event Loop

**Goal:** Create a notify-based event loop for agents that doesn't poll.

**File:** `crates/agent_pool/src/agent.rs` (new file)

```rust
//! Agent-side event loop for waiting on tasks.

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc;

use crate::transport::Transport;
use crate::{AGENTS_DIR, RESPONSE_FILE, TASK_FILE};

/// Events the agent cares about.
pub enum AgentEvent {
    /// A file changed in the agent directory.
    FileChanged,
    /// The watcher encountered an error.
    WatchError(notify::Error),
}

/// Create a watcher for an agent directory.
///
/// Returns the watcher (keep alive) and a receiver for events.
pub fn create_agent_watcher(
    agent_dir: &Path,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<AgentEvent>)> {
    let (tx, rx) = mpsc::channel();

    let event_tx = tx.clone();
    let watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| match res {
            Ok(_) => {
                let _ = event_tx.send(AgentEvent::FileChanged);
            }
            Err(e) => {
                let _ = event_tx.send(AgentEvent::WatchError(e));
            }
        },
        Config::default(),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok((watcher, rx))
}

/// Check if a task is ready to be processed.
///
/// Returns true when: task.json exists AND response.json does not.
pub fn is_task_ready(agent_dir: &Path) -> bool {
    let task_file = agent_dir.join(TASK_FILE);
    let response_file = agent_dir.join(RESPONSE_FILE);
    task_file.exists() && !response_file.exists()
}

/// Wait for a task to be ready.
///
/// Blocks until task.json exists and response.json does not.
/// Uses notify for event-driven waiting (no polling).
pub fn wait_for_task(
    agent_dir: &Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    // Check initial state
    if is_task_ready(agent_dir) {
        return Ok(());
    }

    // Wait for file events
    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if is_task_ready(agent_dir) {
                    return Ok(());
                }
            }
            Ok(AgentEvent::WatchError(e)) => {
                return Err(io::Error::new(io::ErrorKind::Other, e));
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel closed",
                ));
            }
        }
    }
}

/// Wait for task.json to be deleted (daemon acknowledged response).
///
/// Blocks until task.json does not exist.
pub fn wait_for_cleanup(
    agent_dir: &Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    let task_file = agent_dir.join(TASK_FILE);

    // Check initial state
    if !task_file.exists() {
        return Ok(());
    }

    // Wait for file events
    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if !task_file.exists() {
                    return Ok(());
                }
            }
            Ok(AgentEvent::WatchError(e)) => {
                return Err(io::Error::new(io::ErrorKind::Other, e));
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel closed",
                ));
            }
        }
    }
}
```

### Task 3: Update CLI to Use Agent Event Loop

**Goal:** Replace polling in CLI with the new event-driven waiting.

**File:** `crates/agent_pool_cli/src/main.rs`

**Before (lines 195-239):**
```rust
fn wait_for_task(
    task_file: &std::path::Path,
    response_file: &std::path::Path,
    name: &str,
) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            // Read and return task
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

**After:**
```rust
use agent_pool::{create_agent_watcher, wait_for_task, wait_for_cleanup, Transport};
use notify::RecursiveMode;

fn run_agent_loop(pool_root: &Path, agent_name: &str) -> Result<(), String> {
    let agent_dir = pool_root.join(AGENTS_DIR).join(agent_name);
    fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;

    // Set up transport and watcher
    let transport = Transport::Directory(agent_dir.clone());
    let (mut watcher, events_rx) = create_agent_watcher(&agent_dir)
        .map_err(|e| e.to_string())?;
    watcher
        .watch(&agent_dir, RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())?;

    loop {
        // Wait for task (no polling!)
        wait_for_task(&agent_dir, &events_rx).map_err(|e| e.to_string())?;

        // Read task
        let raw = transport.read(TASK_FILE).map_err(|e| e.to_string())?;

        // Parse and handle task...
        let response = process_task(&raw)?;

        // Write response
        transport.write(RESPONSE_FILE, &response).map_err(|e| e.to_string())?;

        // Wait for daemon to clean up
        wait_for_cleanup(&agent_dir, &events_rx).map_err(|e| e.to_string())?;
    }
}
```

### Task 4: Update Test Agents to Use Same Code

**Goal:** Test agents use the same event loop as CLI agents.

**File:** `crates/agent_pool/tests/common/mod.rs`

**Before:**
```rust
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

**After:**
```rust
use agent_pool::{create_agent_watcher, wait_for_task, Transport, TASK_FILE, RESPONSE_FILE};

let transport = Transport::Directory(agent_dir.clone());
let (mut watcher, events_rx) = create_agent_watcher(&agent_dir)?;
watcher.watch(&agent_dir, RecursiveMode::NonRecursive)?;

while running_clone.load(Ordering::SeqCst) {
    // Use recv_timeout so we can check the running flag
    match events_rx.recv_timeout(Duration::from_millis(100)) {
        Ok(AgentEvent::FileChanged) => {
            if is_task_ready(&agent_dir) {
                let raw = transport.read(TASK_FILE)?;
                // Process task...
                let response = processor(&raw);
                transport.write(RESPONSE_FILE, &response)?;
            }
        }
        Ok(AgentEvent::WatchError(_)) => break,
        Err(mpsc::RecvTimeoutError::Timeout) => continue,
        Err(mpsc::RecvTimeoutError::Disconnected) => break,
    }
}
```

Note: Test agents still need `recv_timeout` to check the `running` flag, but the key point is they're not **polling the filesystem** - they're waiting on the event channel.

## Implementation Order

1. **Task 1: Move Transport** - Mechanical refactor, no behavior change
2. **Task 2: Create agent module** - New code, can be tested in isolation
3. **Task 3: Update CLI** - Use new code, tests verify it works
4. **Task 4: Update test agents** - Final step, makes tests use same path as CLI

## Open Questions

1. Should we also move the daemon's `create_fs_watcher` to a shared location, or is the agent version sufficient?
2. The test agent uses `recv_timeout` to check the running flag - is there a cleaner pattern?

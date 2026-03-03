# Universal Cancellation Channel

## Motivation

Multiple blocking operations in `agent_pool` need cancellation support:
- Workers waiting for tasks
- Submitters waiting for responses
- Tests that need clean shutdown

Currently these use timeout-based polling, which is error-prone (we've broken it twice).

## Goal

Introduce a universal cancellation pattern using `crossbeam::channel` + `select!` for all blocking operations.

## Blocking Operations That Need Cancellation

### 1. `VerifiedWatcher::wait_for` (core primitive)

**Location:** `verified_watcher.rs:205-261`

The foundation. All file-waiting operations use this. Making it cancellable gives cancellation to everything built on top.

```rust
// Current: timeout-based
watcher.wait_for(&path, Some(Duration::from_secs(5)))?;

// Proposed: cancellation channel
watcher.wait_for(&path, Some(&cancel_rx))?;
```

### 2. `wait_for_task` (workers)

**Location:** `worker.rs:46-67`

Workers waiting for task assignments from the daemon.

```rust
// Current: callers poll with timeout
while running.load(Ordering::SeqCst) {
    let Ok(task) = wait_for_task(&pool, None, Some(Duration::from_millis(500))) else {
        continue;
    };
}

// Proposed: pass cancellation channel
let task = wait_for_task(&pool, None, Some(&cancel_rx))?;
```

### 3. `submit_file_with_timeout` (file-based submission)

**Location:** `submit/file.rs:68-109`

Submitters waiting for task responses via filesystem.

```rust
// Current: uses VerifiedWatcher with timeout
watcher.wait_for(&response_path, Some(timeout))?;

// Proposed: cancellation channel
let response = submit_file(&pool, &payload, Some(&cancel_rx))?;
```

### 4. `submit` (socket-based submission)

**Location:** `submit/socket.rs:28-67`

Socket submission has two blocking phases:
1. Wait for pool ready (uses `VerifiedWatcher`)
2. Socket read (blocking I/O)

Phase 1 gets cancellation for free via `VerifiedWatcher`. Phase 2 (socket read) is trickier - would need non-blocking socket with select, or accept that socket submissions can't be cancelled mid-read.

```rust
// Current
watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;
// ... blocking socket read ...

// Proposed: at least cancel the wait_for part
watcher.wait_for(&status_path, Some(&cancel_rx))?;
// Socket read still blocks (acceptable for now)
```

### 5. `wait_for_pool_ready` (pool startup)

**Location:** `submit/mod.rs:27-46`

Polls for status file with 10ms sleep. Special case: can't use watcher because daemon clears pool directory on startup (races with watcher setup).

```rust
// Current: polls with sleep
while !status_path.exists() {
    thread::sleep(Duration::from_millis(10));
}

// Options:
// A) Keep polling but check cancellation channel with try_recv()
// B) Use watcher anyway, handle the race differently
// C) Leave as-is (short timeout, rarely needs cancellation)
```

### 6. `VerifiedWatcher::into_receiver` (canary verification)

**Location:** `verified_watcher.rs:272-314`

Waits for all canary directories to be verified. Usually quick, but could theoretically hang.

```rust
// Current: timeout-based
let (watcher, rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;

// Proposed: cancellation channel
let (watcher, rx) = verified_watcher.into_receiver(Some(&cancel_rx))?;
```

## Proposed API

### Core Type

```rust
use crossbeam::channel::Receiver;

/// A channel that signals cancellation when a message is received.
pub type CancelRx = Receiver<()>;
```

### VerifiedWatcher

```rust
impl VerifiedWatcher {
    /// Wait for a file to appear, with optional cancellation.
    pub fn wait_for(
        &mut self,
        target: &Path,
        cancel: Option<&CancelRx>,
    ) -> io::Result<()>;

    /// Consume watcher after verification, with optional cancellation.
    pub fn into_receiver(
        self,
        cancel: Option<&CancelRx>,
    ) -> io::Result<(RecommendedWatcher, Receiver<notify::Event>)>;
}
```

### Worker API

```rust
/// Wait for a task assignment, with optional cancellation.
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel: Option<&CancelRx>,
) -> io::Result<TaskAssignment>;
```

### Submission API

```rust
/// Submit via file protocol, with optional cancellation.
pub fn submit_file(
    root: impl AsRef<Path>,
    payload: &Payload,
    cancel: Option<&CancelRx>,
) -> io::Result<Response>;

/// Submit via socket, with optional cancellation (only for wait_for phase).
pub fn submit(
    root: impl AsRef<Path>,
    payload: &Payload,
    cancel: Option<&CancelRx>,
) -> io::Result<Response>;
```

## Implementation

### 1. Add crossbeam dependency

```toml
# crates/agent_pool/Cargo.toml
[dependencies]
crossbeam = "0.8"
```

### 2. Update VerifiedWatcher internals

Change from `std::sync::mpsc` to `crossbeam::channel`:

```rust
use crossbeam::channel::{self, Receiver, Sender};

struct WatcherState {
    rx: Receiver<notify::Event>,  // Changed from mpsc::Receiver
    remaining_canaries: Vec<CanaryGuard>,
}
```

### 3. Implement wait_for with select

```rust
pub fn wait_for(
    &mut self,
    target: &Path,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    let never = channel::never();
    let cancel = cancel.unwrap_or(&never);

    loop {
        crossbeam::select! {
            recv(self.state.rx) -> event => {
                match event {
                    Ok(e) => {
                        for path in &e.paths {
                            if path == target {
                                return Ok(());
                            }
                        }
                        // Also check exists() for edge cases
                        if target.exists() {
                            return Ok(());
                        }
                    }
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "watcher disconnected",
                        ));
                    }
                }
            }
            recv(cancel) -> _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            // Canary retry on timeout
            default(Duration::from_millis(100)) => {
                if target.exists() {
                    return Ok(());
                }
                for canary in &mut self.state.remaining_canaries {
                    canary.retry()?;
                }
            }
        }
    }
}
```

### 4. Update all callers

Each caller creates a channel and passes the receiver:

```rust
// Test agent example
let (cancel_tx, cancel_rx) = crossbeam::channel::bounded(1);

let handle = thread::spawn(move || {
    loop {
        match wait_for_task(&pool, None, Some(&cancel_rx)) {
            Ok(task) => process(task),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
            Err(e) => { eprintln!("error: {e}"); break; }
        }
    }
});

// To stop
let _ = cancel_tx.send(());
```

## Migration Strategy

1. **Phase 1:** Add `crossbeam` dependency, change `VerifiedWatcher` internals
2. **Phase 2:** Add `cancel` parameter to `wait_for` (default `None` for backwards compat)
3. **Phase 3:** Update `wait_for_task` to accept and pass through cancellation
4. **Phase 4:** Update submission functions
5. **Phase 5:** Update test agents to use channels instead of AtomicBool + timeout
6. **Phase 6:** Remove timeout parameter from functions (or keep for actual timeouts separate from cancellation)

## Open Questions

1. **Timeout vs cancellation:** Should we keep timeout as a separate concept? A timeout is "give up after N seconds", while cancellation is "stop immediately when signaled". Could have both:
   ```rust
   fn wait_for(&mut self, target: &Path, timeout: Option<Duration>, cancel: Option<&CancelRx>)
   ```

2. **Socket read cancellation:** The socket-based `submit()` blocks on socket read after connecting. Full cancellation would require non-blocking sockets + select. Worth it?

3. **Error types:** Use `io::ErrorKind::Interrupted` for cancellation, or define a custom error type?

## Testing

- Unit: `wait_for` returns `Interrupted` when cancel signal sent
- Unit: `wait_for` continues working after spurious wakeups
- Integration: Test agent stops within 100ms of `stop()` call (not 500ms timeout)
- Integration: File submission can be cancelled mid-wait
- Negative: Ensure cancellation doesn't leave dangling files/resources

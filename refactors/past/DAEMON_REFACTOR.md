# Daemon Event Loop Refactor: Polling → Wake Channel

## Goal

Replace the current poll-based event loop with a wake channel pattern. The main loop blocks until any event source has work, then drains all sources. Zero CPU waste.

## Current Architecture

### Event Loop Location
`crates/agent_pool/src/daemon/wiring.rs` - `io_loop()`

### Current Flow (Polling)

```rust
let poll_timeout = Duration::from_millis(100);

loop {
    // Non-blocking socket accept
    if let Some((raw, stream)) = accept_socket_task(listener)? { ... }

    // Block with 100ms timeout for FS events
    match fs_events.recv_timeout(poll_timeout) { ... }

    // Drain effects (non-blocking)
    while let Ok(effect) = effects_rx.try_recv() { ... }
}
```

### Problem

**CPU waste**: Wakes every 100ms even when idle, checking each source sequentially.

---

## Target Architecture

```rust
let (wake_tx, wake_rx) = mpsc::channel::<()>();

// Each event source runs in its own thread, sends to its channel, then pings wake

loop {
    wake_rx.recv()?;  // Block until any source has something

    // Drain all sources (non-blocking)
    while let Ok(event) = fs_rx.try_recv() { ... }
    while let Ok((raw, stream)) = socket_rx.try_recv() { ... }
    while let Ok(effect) = effects_rx.try_recv() { ... }
}
```

---

## Tasks

### Task 1: Create wake channel

**File:** `wiring.rs` in `run_daemon()`

```rust
let (wake_tx, wake_rx) = mpsc::channel::<()>();
```

### Task 2: Update FS watcher to ping wake

**File:** `wiring.rs` in `create_fs_watcher()`

Current:
```rust
fn create_fs_watcher(root: &Path) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })?;
    // ...
}
```

New signature:
```rust
fn create_fs_watcher(
    root: &Path,
    wake_tx: mpsc::Sender<()>,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)>
```

New body:
```rust
let (tx, rx) = mpsc::channel();
let mut watcher = notify::recommended_watcher(move |event| {
    if let Ok(event) = event {
        let _ = tx.send(event);
        let _ = wake_tx.send(());  // Wake main loop
    }
})?;
```

### Task 3: Create socket accept thread

**File:** `wiring.rs`

Currently, socket accept is inline in `io_loop()`. Move it to a dedicated thread.

New function:
```rust
fn spawn_socket_accept_thread(
    listener: Listener,
    socket_tx: mpsc::Sender<(RawSubmission, Stream)>,
    wake_tx: mpsc::Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match accept_socket_task(&listener) {
                Ok(Some((raw, stream))) => {
                    let _ = socket_tx.send((raw, stream));
                    let _ = wake_tx.send(());
                }
                Ok(None) => {
                    // Non-blocking returned nothing, shouldn't happen in blocking mode
                    // But if it does, just continue
                }
                Err(e) => {
                    error!("Socket accept error: {}", e);
                    break;
                }
            }
        }
    })
}
```

**Note:** `accept_socket_task` currently does non-blocking accept. Change to blocking, or use a different approach. The simplest: just call `listener.accept()` directly (blocking).

### Task 4: Wire effects channel to wake

**File:** `wiring.rs`

The event loop thread sends effects. It needs to ping wake after each send.

Current in `run_event_loop_with_shutdown()`:
```rust
for effect in effects {
    effects_tx.send(effect)?;
}
```

New:
```rust
for effect in effects {
    effects_tx.send(effect)?;
    let _ = wake_tx.send(());
}
```

This requires passing `wake_tx` to `run_event_loop_with_shutdown()`.

### Task 5: Update io_loop to use wake pattern

**File:** `wiring.rs` in `io_loop()`

New signature:
```rust
fn io_loop(
    wake_rx: mpsc::Receiver<()>,
    fs_events: mpsc::Receiver<notify::Event>,
    socket_rx: mpsc::Receiver<(RawSubmission, Stream)>,
    effects_rx: mpsc::Receiver<Effect>,
    events_tx: mpsc::Sender<Event>,
    // ... other params
) -> io::Result<()>
```

New body:
```rust
loop {
    // Block until any source has work
    if wake_rx.recv().is_err() {
        // All senders dropped, shutdown
        break;
    }

    // Drain FS events
    while let Ok(event) = fs_events.try_recv() {
        handle_fs_event(&event, &events_tx, ...)?;
    }

    // Drain socket submissions
    while let Ok((raw, stream)) = socket_rx.try_recv() {
        handle_socket_submission(raw, stream, &events_tx, ...)?;
    }

    // Drain effects
    while let Ok(effect) = effects_rx.try_recv() {
        execute_effect(effect, ...)?;
    }
}

Ok(())
```

### Task 6: Update run_daemon to wire everything together

**File:** `wiring.rs` in `run_daemon()`

```rust
pub fn run_daemon(root: &Path, config: DaemonConfig) -> io::Result<Infallible> {
    // ... setup ...

    // Create wake channel
    let (wake_tx, wake_rx) = mpsc::channel();

    // Create FS watcher (now takes wake_tx)
    let (watcher, fs_events) = create_fs_watcher(&root, wake_tx.clone())?;

    // Create socket listener and accept thread
    let listener = create_socket_listener(&socket_path)?;
    let (socket_tx, socket_rx) = mpsc::channel();
    let _socket_thread = spawn_socket_accept_thread(listener, socket_tx, wake_tx.clone());

    // Create event loop channels
    let (events_tx, events_rx) = mpsc::channel();
    let (effects_tx, effects_rx) = mpsc::channel();

    // Spawn event loop thread (now takes wake_tx for effect notifications)
    let event_loop_handle = thread::spawn(move || {
        run_event_loop_with_shutdown(events_rx, effects_tx, wake_tx, signals)
    });

    // Run IO loop
    io_loop(wake_rx, fs_events, socket_rx, effects_rx, events_tx, ...)?;

    unreachable!()
}
```

---

## What Stays the Same

- **Core state machine** (`core.rs`)
- **Effect execution** (`io.rs`)
- **Path categorization** (`path_category.rs`)
- **Public API** signatures

---

## Estimated Scope

- ~100 lines changed in `wiring.rs`
- No new dependencies
- No changes to core logic

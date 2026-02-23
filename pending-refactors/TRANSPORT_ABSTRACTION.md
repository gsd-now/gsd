# Transport Abstraction Plan

## Overview

Communication between components involves two orthogonal choices:

1. **Payload format:** How is the content delivered?
   - **Inline:** The message contains the actual content
   - **File reference:** The message contains a path; recipient reads the file

2. **Notification mechanism:** How does the recipient know there's a message?
   - **Socket:** RPC call (direct connection, immediate)
   - **FS events:** Write a file to a watched location; recipient notices via file watcher

This 2x2 grid applies to both:
- **Submission:** Submitter → Daemon (and response back)
- **Task handling:** Daemon → Agent (and response back)

## The 2x2 Grid

|                    | Inline                          | File Reference                     |
|--------------------|---------------------------------|------------------------------------|
| **Socket**         | Send content over socket        | Send path over socket; recipient reads file |
| **FS Events**      | Write content to notification file | Write path to notification file; recipient reads referenced file |

### Why FS Events Exist

Socket = RPC. Direct call to daemon.

FS events = fallback when sockets are blocked (sandboxed environments). Create a file in a watched directory to trigger an event. The daemon's file watcher notices and processes it.

### The "Double Read" with FS Events + File Reference

When using FS events notification with file reference payload:

1. Submitter writes `pending/<uuid>/task.json` containing `{"file": "/path/to/actual/task.json"}`
2. Daemon reads `pending/<uuid>/task.json` (read #1) → gets the path
3. Daemon reads `/path/to/actual/task.json` (read #2) → gets the content

This is not waste—the first read answers "what event happened?" and the second gets the actual content. The notification file is tiny (just metadata + path).

With socket notification, there's only one read because the RPC call itself carries the path.

## Applies To Both Directions

### Submission (Submitter ↔ Daemon)

```
Submitter                 Daemon
   |                         |
   |---- submit task ------->|   (notification + payload)
   |                         |
   |<--- response -----------|   (notification + payload)
   |                         |
```

**CLI flags:**
- `--data "content"` = inline payload
- `--file /path/to/task.json` = file reference payload
- `--notify socket` (default) or `--notify file` = notification mechanism

### Task Handling (Daemon ↔ Agent)

```
Daemon                    Agent
   |                         |
   |---- dispatch task ----->|   (notification + payload)
   |                         |
   |<--- response -----------|   (notification + payload)
   |                         |
```

**Agent commands:**
- `register` - Initial connection, receives first task (health check)
- `next_task` - Submit response to previous task, receive next task

**CLI flags (same as submission):**
- `--data "response"` = inline payload
- `--file /path/to/response.json` = file reference payload
- `--notify socket` (default) or `--notify file` = notification mechanism

**Agent loop:**
```bash
task=$(agent_pool register --pool $POOL --name $NAME)
while true; do
    response=$(process "$task")
    task=$(agent_pool next_task --pool $POOL --name $NAME --data "$response")
done
```

After `register`, every `next_task` call is symmetric: "here's my response, give me next task."

## Sandbox Restrictions

Sandboxes block the `connect()` syscall, which blocks socket notification.

**What sandboxes block:** Socket notification (can't do RPC)
**What sandboxes allow:** FS events notification (just file read/write)

The payload format (inline vs file reference) is unaffected by sandbox restrictions. A sandboxed agent can use either payload format—they just can't use socket notification.

| Environment | Socket Notification | FS Events Notification |
|-------------|---------------------|------------------------|
| Normal      | ✓                   | ✓                      |
| Sandboxed   | ✗ (blocked)         | ✓                      |

## Current Implementation

Currently we conflate these concepts:

```rust
// "submit" = socket notification + inline payload
pub fn submit(root: &Path, input: &str) -> io::Result<Response>;

// "submit_file" = fs events notification + inline payload
// (misleading name—it's about notification, not payload format)
pub fn submit_file(root: &Path, input: &str) -> io::Result<Response>;
```

The current `--file` flag in the CLI reads the file locally and sends content—it doesn't send a file reference.

**Socket submission is broken:** The client side (`submit()`) sends data and waits for response. The daemon side (`accept_socket_task()`) reads the data but then drops the connection without ever sending a response. Clients hang forever.

## Target Design

Separate the two axes clearly:

```rust
/// What content are we sending?
enum Payload {
    Inline(String),
    FileReference(PathBuf),
}

/// How do we communicate with the daemon?
/// Used identically for submission and agent responses.
enum NotifyMethod {
    Socket { socket_path: PathBuf },
    FsEvents { dir: PathBuf },
}

impl NotifyMethod {
    /// Send message to daemon, wait for response.
    /// Same logic for submission and agent task completion.
    fn send_and_receive(&self, payload: &Payload) -> io::Result<String> {
        match self {
            Self::Socket { socket_path } => {
                // Connect to socket, write payload, read response
            }
            Self::FsEvents { dir } => {
                // Write payload to dir/task.json
                // Poll for dir/response.json
                // Read and return response
            }
        }
    }
}
```

**CLI:**
```bash
# Submission
agent_pool submit_task --pool $POOL --data "content"              # inline
agent_pool submit_task --pool $POOL --file /path/to/task.json     # file reference
agent_pool submit_task --pool $POOL --data "content" --notify file  # sandboxed

# Agent registration (first call, gets initial task/health check)
agent_pool register --pool $POOL --name $NAME

# Agent next task (submit response, get next task)
agent_pool next_task --pool $POOL --name $NAME --data "response"
agent_pool next_task --pool $POOL --name $NAME --file /path/to/response.json
agent_pool next_task --pool $POOL --name $NAME --data "response" --notify file  # sandboxed
```

**Unified flag names:**
- `--data "content"` = inline payload (same name for submission and agent response)
- `--file /path` = file reference payload
- `--notify socket|file` = notification mechanism (default: socket)

---

## Implementation: Socket Support for Submissions

### Current State

**Client side (works):**
```rust
// client/submit.rs
pub fn submit(root: &Path, input: &str) -> io::Result<Response> {
    let mut stream = Stream::connect(socket_path)?;
    writeln!(stream, "{}", input.len())?;      // Send length
    stream.write_all(input.as_bytes())?;       // Send content
    // ... waits for response on same stream
}
```

**Daemon side (broken):**
```rust
// daemon/wiring.rs - accept_socket_task()
fn accept_socket_task(listener: &Listener) -> io::Result<Option<(String, PathBuf)>> {
    match listener.accept() {
        Ok(stream) => {
            // Read length and content from stream
            // ...
            warn!("socket-based submissions not yet supported, task ignored");
            drop(stream);  // <-- Stream dropped! Client hangs forever.
            Ok(None)
        }
    }
}
```

### What Needs to Happen

1. **Keep the stream alive** - Don't drop it after reading the task
2. **Store the stream** - Associate it with the task ID
3. **Write response** - When task completes, write response back to the stream

### Detailed Implementation Plan

#### Step 1: Add Socket Transport Variant

In `daemon/io.rs`, add `Socket` variant to existing `Transport` enum:

```rust
pub(super) enum Transport {
    Directory(PathBuf),
    Socket(Stream),  // NEW
}
```

The `ExternalTaskMap` (which is `TransportMap<ExternalTaskId>`) already stores `(Transport, ExternalTaskData)`. For socket tasks, we store `(Transport::Socket(stream), ExternalTaskData)` - same pattern as directory-based tasks.

**Note:** `Stream` from interprocess doesn't implement `Debug`, so we'll need a manual `Debug` impl or wrapper.

#### Step 2: Update TransportMap for Socket Registration

`TransportMap` currently has `path_to_id` reverse lookup that assumes paths. For socket transports:
- Skip the `path_to_id` registration (socket tasks aren't discovered via filesystem)
- Add a `register_socket()` method that only inserts into `entries`, not `path_to_id`

```rust
impl<Id: TransportId> TransportMap<Id> {
    /// Register a socket-based transport (no path lookup).
    pub fn register_socket(&mut self, id: Id, stream: Stream, data: Id::Data) {
        self.entries.insert(id, (Transport::Socket(stream), data));
        // No path_to_id entry - socket tasks don't have paths
    }
}
```

#### Step 3: Update accept_socket_task

```rust
fn accept_socket_task(listener: &Listener) -> io::Result<Option<(String, Stream)>> {
    match listener.accept() {
        Ok(stream) => {
            let mut reader = BufReader::new(&stream);

            let mut len_line = String::new();
            reader.read_line(&mut len_line)?;
            let len: usize = len_line.trim().parse().map_err(|_| ...)?;

            let mut content = vec![0u8; len];
            reader.read_exact(&mut content)?;
            let content = String::from_utf8(content).map_err(|_| ...)?;

            // Return both content and stream (don't drop it!)
            Ok(Some((content, stream)))
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}
```

**Problem:** We're borrowing `stream` for `reader`, then need to return `stream`. Need to handle the borrow carefully - either:
- Use `into_inner()` to get the stream back from the BufReader
- Read without BufReader (manual buffering)
- Clone the stream (if supported)

#### Step 4: Register Socket Task

In `io_loop`:

```rust
if let Some((content, stream)) = accept_socket_task(listener)? {
    let external_id = task_id_allocator.allocate_external();

    // Register with Transport::Socket (no path)
    external_task_map.register_socket(
        external_id,
        stream,
        ExternalTaskData {
            content,
            timeout: io_config.default_task_timeout,
        },
    );

    let _ = events_tx.send(Event::TaskSubmitted {
        task_id: TaskId::External(external_id),
    });
}
```

#### Step 5: Write Response via Transport

Update `ExternalTaskMap::finish()` to handle both transport types:

```rust
impl ExternalTaskMap {
    pub fn finish(&mut self, id: ExternalTaskId, response: &str) -> io::Result<ExternalTaskData> {
        let (transport, data) = self.remove(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;

        match transport {
            Transport::Directory(path) => {
                fs::write(path.join(RESPONSE_FILE), response)?;
            }
            Transport::Socket(mut stream) => {
                writeln!(stream, "{}", response.len())?;
                stream.write_all(response.as_bytes())?;
                stream.flush()?;
            }
        }

        Ok(data)
    }
}
```

This handles both `TaskCompleted` and `TaskFailed` - both just call `finish()` with the appropriate response content.

### Complications

1. **BufReader ownership** - Reading from socket uses BufReader which borrows the stream. Need to get stream back after reading (use `into_inner()` or read without BufReader).

2. **Stream doesn't implement Debug** - `interprocess::local_socket::Stream` doesn't derive Debug. Need manual Debug impl for Transport or skip Debug for Socket variant.

3. **Cleanup on drop** - If daemon crashes or task is never completed, socket closes automatically when dropped (correct behavior).

---

## Implementation Phases

### Phase 1: Fix Socket Submissions (daemon-side)

**Goal:** Make `submit()` work end-to-end.

1. Add `Transport::Socket(Stream)` variant
2. Add `TransportMap::register_socket()` method (no path lookup)
3. Update `accept_socket_task()` to return the stream
4. Update `ExternalTaskMap::finish()` to write to socket or file based on transport type
5. Test: `agent_pool submit_task --input '...'` should work

### Phase 2: Clarify CLI Flags

**Goal:** Clean up CLI to match the 2x2 grid.

1. Rename `--file` to `--input-file` (reads file, sends content inline)
2. Add `--data` as alias for `--input` (inline content)
3. Add `--notify socket|file` flag (default: socket)
4. Update help text to explain the distinction

### Phase 3: Add File Reference Support

**Goal:** Support sending paths instead of content.

1. Add `--file-ref /path` flag that sends the path (not content)
2. Daemon reads the referenced file when processing task
3. Works with both socket and FS events notification

---

## Future: Auto-Discovery of Notification Mechanism

Currently, users must explicitly pass `--notify file` in sandboxed environments. Auto-discovery could detect when sockets are blocked and fall back automatically.

**Options (all deferred):**
1. **Try socket, fall back to file** - latency cost on every call
2. **Cache in environment variable** - `AGENT_POOL_NOTIFY=file`
3. **Cache in pool directory** - write `.notify-method` file
4. **Don't auto-discover** - user explicitly passes `--notify file`

For initial implementation, option 4 (explicit flag) is simplest.

---

## Future: Remote Pools

A third notification mechanism could be added for remote daemons:

| Notification     | Description                    | File Reference Works? |
|------------------|--------------------------------|----------------------|
| Local Socket     | Unix socket on same machine    | ✓ (shared filesystem) |
| FS Events        | File watcher on same machine   | ✓ (shared filesystem) |
| Remote Socket    | TCP socket to different machine| ✗ (no shared filesystem) |

For remote sockets with file reference payload, the CLI would read the file and send content inline (automatic fallback).

This is out of scope for now but the design accommodates it.

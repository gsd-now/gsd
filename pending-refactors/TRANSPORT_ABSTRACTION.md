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

In `daemon/io.rs`:

```rust
pub(super) enum Transport {
    Directory(PathBuf),
    Socket(Stream),  // NEW
}

impl Transport {
    pub fn write_response(&mut self, content: &str) -> io::Result<()> {
        match self {
            Transport::Directory(path) => {
                fs::write(path.join(RESPONSE_FILE), content)
            }
            Transport::Socket(stream) => {
                writeln!(stream, "{}", content.len())?;
                stream.write_all(content.as_bytes())?;
                stream.flush()
            }
        }
    }
}
```

**Note:** `Stream` from interprocess doesn't implement `Debug`, so we'll need `#[derive(Debug)]` workarounds or manual impl.

#### Step 2: Store Socket in ExternalTaskMap

Currently `ExternalTaskData` stores:
```rust
pub(super) struct ExternalTaskData {
    pub content: String,
    pub timeout: Duration,
}
```

For socket tasks, we also need to store the stream. Options:

**Option A: Add stream to ExternalTaskData**
```rust
pub(super) struct ExternalTaskData {
    pub content: String,
    pub timeout: Duration,
    pub response_stream: Option<Stream>,  // None for file-based
}
```

**Option B: Use Transport directly**
Already have `TransportMap<ExternalTaskId>` which stores `(Transport, ExternalTaskData)`. Change `Transport::Directory` to only be used for agents, add a new map for socket submissions.

**Option C: Separate SocketTaskMap**
Keep file-based and socket-based tasks in separate maps.

**Recommendation:** Option A is simplest. The stream is just another piece of data associated with the task.

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

#### Step 4: Register Socket Task with Stream

In `io_loop`:

```rust
if let Some((content, stream)) = accept_socket_task(listener)? {
    let external_id = task_id_allocator.allocate_external();
    // Store stream with the task
    if external_task_map.register(
        external_id,
        PathBuf::new(),  // No path for socket tasks
        ExternalTaskData {
            content,
            timeout: io_config.default_task_timeout,
            response_stream: Some(stream),  // NEW
        },
    ) {
        let _ = events_tx.send(Event::TaskSubmitted {
            task_id: TaskId::External(external_id),
        });
    }
}
```

**Issue:** `register()` currently requires a `PathBuf`. For socket tasks, there's no path. Options:
- Make path optional
- Use a sentinel value
- Have separate registration method for socket tasks

#### Step 5: Write Response to Socket

In `execute_effect`, when handling `TaskCompleted` for external tasks:

```rust
TaskId::External(external_id) => {
    let response = agent_map.read_from(agent_id, RESPONSE_FILE)?;

    // Get the task data (includes stream if socket-based)
    let task_data = external_task_map.remove(external_id)?;

    if let Some(mut stream) = task_data.response_stream {
        // Socket-based: write response to stream
        writeln!(stream, "{}", response.len())?;
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
    } else {
        // File-based: write response to file (existing behavior)
        external_task_map.finish(external_id, &response)?;
    }
}
```

#### Step 6: Handle Socket Task Failures

When a socket task times out or fails:

```rust
Effect::TaskFailed { task_id } => {
    match task_id {
        TaskId::External(external_id) => {
            let error = json!({"status": "NotProcessed", "reason": "AgentTimeout"});

            if let Some(task_data) = external_task_map.remove(external_id) {
                if let Some(mut stream) = task_data.response_stream {
                    // Write error to socket
                    let error_str = error.to_string();
                    let _ = writeln!(stream, "{}", error_str.len());
                    let _ = stream.write_all(error_str.as_bytes());
                    let _ = stream.flush();
                } else {
                    // Write error to file (existing behavior)
                }
            }
        }
    }
}
```

### Complications

1. **BufReader ownership** - Reading from socket uses BufReader which borrows the stream. Need to get stream back after reading.

2. **Stream doesn't implement Debug** - `interprocess::local_socket::Stream` doesn't derive Debug. Need manual Debug impl or wrapper.

3. **Path requirement in TransportMap** - Current design assumes every task has a path. Socket tasks don't.

4. **Thread safety** - Socket stream needs to be accessible when writing response, potentially from different context than where it was created.

5. **Cleanup on drop** - If daemon crashes or task is never completed, socket should be closed cleanly.

### Simplified Alternative

Instead of storing the stream in the task map, handle socket tasks synchronously:

```rust
fn handle_socket_submission(stream: Stream, daemon: &Daemon) -> io::Result<()> {
    // Read task from stream
    let task = read_task(&stream)?;

    // Submit task and wait for completion (blocking)
    let response = daemon.submit_and_wait(task)?;

    // Write response
    write_response(&stream, &response)?;

    Ok(())
}
```

**Problem:** This blocks the I/O thread. Would need to spawn a thread per socket connection, or use async.

### Recommended Approach

1. Start with the full async-storage approach (store stream in task data)
2. Add `Option<Stream>` to `ExternalTaskData`
3. Handle the BufReader ownership issue by reading into owned buffer first
4. Add a wrapper type for Stream that implements Debug
5. Update `finish()` to handle socket responses

---

## Implementation Phases

### Phase 1: Fix Socket Submissions (daemon-side)

**Goal:** Make `submit()` work end-to-end.

1. Add `response_stream: Option<Stream>` to `ExternalTaskData`
2. Update `accept_socket_task()` to return the stream
3. Store stream when registering socket task
4. Write response to stream in `execute_effect(TaskCompleted)`
5. Write error to stream in `execute_effect(TaskFailed)`
6. Test: `agent_pool submit_task --input '...'` should work

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

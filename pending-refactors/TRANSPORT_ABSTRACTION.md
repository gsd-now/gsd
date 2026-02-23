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

**Note:** Agent socket responses will panic for now (not yet implemented), but the code structure supports it. The goal is one enum handling both submission and agent responses identically.

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

## Independent Migrations

The two axes can be migrated independently:

### Payload Format Migration

Add `--file` flag that sends a path (file reference) instead of reading and inlining content. Works with either notification mechanism.

**Before:** `--file` reads file, sends content (inline)
**After:** `--file` sends the path (file reference)

### Notification Mechanism Migration

Already exists (`submit` vs `submit_file`), but rename for clarity:
- `--notify socket` (default)
- `--notify file`

### Migration Order

Either order works:
1. Fix `--file` to be true file reference, then add `--notify` flag
2. Add `--notify` flag first, then fix `--file` semantics

## Edge Case: Inaccessible Filesystems

Currently we assume the submitter, daemon, and agents all share the same filesystem. File reference works because everyone can read the path.

In a hypothetical future where the daemon can't access the submitter's filesystem:
- File reference wouldn't work directly
- The CLI would need to detect this and fall back to reading the file and sending inline

This is similar to the "remote socket" case discussed below. For now, we assume shared filesystem access.

**Implementation note:** If we ever need to handle this case, the CLI (not the daemon) should detect it and automatically convert file reference to inline. The daemon shouldn't need to know about this complexity.

---

## Implementation Phases

### Phase 1: Clarify Payload Format

1. Rename current `--file` behavior to something explicit (it's inline, not file reference)
2. Add true file reference support: `--file` sends the path, daemon reads the file
3. Update both submission and task completion to support both payload formats

### Phase 2: Clarify Notification Mechanism

1. Rename `submit()` and `submit_file()` to reflect notification mechanism
2. Add `--notify socket|file` CLI flag
3. Default to socket, fall back to file in sandboxed environments

### Phase 3: Daemon Architecture

The daemon should be split into three layers:

```
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 3: I/O                                 │
│                                                                  │
│  - socket.accept(), fs_watcher.next()                           │
│  - Parse JSON, resolve file references                          │
│  - Send responses (socket write, file write)                    │
│                                                                  │
│  Sends events to Layer 2 via channel                            │
│  Receives "send response" effects from Layer 2                  │
└────────────────────────────────┬────────────────────────────────┘
                                 │ Event channel
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 2: Event Loop                          │
│                                                                  │
│  - Holds PoolState                                              │
│  - Receives events from Layer 3                                 │
│  - Calls step(state, event) in a loop                           │
│  - Processes returned effects (delegates I/O back to Layer 3)   │
└────────────────────────────────┬────────────────────────────────┘
                                 │ step(state, event)
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 1: Pure State Machine                  │
│                                                                  │
│  fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>)
│                                                                  │
│  - No I/O, no channels, no async                                │
│  - Pure function: state + event → new state + effects           │
│  - Completely testable with plain function calls                │
└─────────────────────────────────────────────────────────────────┘
```

**Layer 1: Pure State Machine**

```rust
/// Pool state - all the data the daemon tracks
struct PoolState {
    /// Tasks waiting to be assigned
    pending_tasks: VecDeque<Task>,
    /// Registered agents and their status
    agents: HashMap<String, AgentState>,
    /// Tasks currently being processed
    in_flight: HashMap<TaskId, InFlightTask>,
}

/// Events that can happen
enum Event {
    /// New task submitted
    Submission { content: String, respond_to: ResponderId },
    /// Agent registered
    AgentRegister { name: String, respond_to: ResponderId },
    /// Agent responded to previous task
    AgentResponse { name: String, response: String, respond_to: ResponderId },
    /// Health check timer fired
    HealthCheckTick,
}

/// Effects to perform (I/O actions)
enum Effect {
    /// Send response to a submitter or agent
    SendResponse { to: ResponderId, content: String },
    /// Schedule health check for agent
    ScheduleHealthCheck { agent: String, delay: Duration },
}

/// Pure function - no I/O, fully testable
fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::Submission { content, respond_to } => {
            let mut state = state;
            let task = Task::new(content, respond_to);
            state.pending_tasks.push_back(task);
            // Try to dispatch immediately if agent available
            let effects = try_dispatch(&mut state);
            (state, effects)
        }
        Event::AgentRegister { name, respond_to } => {
            // Register agent, send health check
            // ...
        }
        // ...
    }
}
```

**Layer 2: Event Loop**

```rust
/// Holds state, receives events, calls step(), processes effects
async fn event_loop(
    mut state: PoolState,
    mut events: mpsc::Receiver<Event>,
    io: IoHandle,  // For sending effects back to Layer 3
) {
    while let Some(event) = events.recv().await {
        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
            io.execute(effect).await;
        }
    }
}
```

**Layer 3: I/O**

```rust
/// All actual I/O happens here
async fn io_layer(
    events_tx: mpsc::Sender<Event>,
    mut effects_rx: mpsc::Receiver<Effect>,
    socket: UnixListener,
    fs_watcher: FsWatcher,
) {
    loop {
        select! {
            // Receive from socket or fs, parse, send event
            conn = socket.accept() => {
                let msg = parse_message(conn).await;
                let event = to_event(msg);
                events_tx.send(event).await;
            }
            fs_event = fs_watcher.next() => {
                let event = parse_fs_event(fs_event).await;
                events_tx.send(event).await;
            }

            // Execute effects (send responses)
            effect = effects_rx.recv() => {
                match effect {
                    Effect::SendResponse { to, content } => {
                        send_response(to, content).await;
                    }
                    // ...
                }
            }
        }
    }
}
```

**Benefits:**

1. **Layer 1 is trivially testable** - just call `step()` with inputs, assert outputs
2. **No I/O in core logic** - all I/O is in Layer 3
3. **Clean separation** - each layer has one job
4. **Easy to add transports** - only Layer 3 changes
5. **State machine is explicit** - easy to reason about state transitions

---

## Dependencies

**Phase 1 and 2 can start now** - CLI changes only, no daemon changes.

**Phase 3** requires more thought on the daemon event loop. May benefit from async (tokio) but not required.

---

## Future: Auto-Discovery of Notification Mechanism

Currently, users must explicitly pass `--notify file` in sandboxed environments. Auto-discovery could detect when sockets are blocked and fall back automatically.

**Challenge:** Submit calls are stateless. Each `submit_task` invocation is independent, so there's no natural place to cache "sockets work" or "sockets are blocked."

**Options (all deferred):**
1. **Try socket, fall back to file** - latency cost on every call in sandboxed environments (try connect, fail, then use file)
2. **Cache in environment variable** - CLI sets `AGENT_POOL_NOTIFY=file` after first socket failure; subsequent calls read this
3. **Cache in pool directory** - write `.notify-method` file after first successful/failed attempt
4. **Don't auto-discover** - user explicitly passes `--notify file` (current approach)

For initial implementation, option 4 (explicit flag) is simplest. Auto-discovery can be added later if the UX burden of `--notify file` proves annoying.

---

## Future: Remote Pools

A third notification mechanism could be added for remote daemons:

| Notification     | Description                    | File Reference Works? |
|------------------|--------------------------------|----------------------|
| Local Socket     | Unix socket on same machine    | ✓ (shared filesystem) |
| FS Events        | File watcher on same machine   | ✓ (shared filesystem) |
| Remote Socket    | TCP socket to different machine| ✗ (no shared filesystem) |

For remote sockets with file reference payload, the CLI would read the file and send content inline (automatic fallback). From the user's perspective, `--file` always works—the CLI handles the details.

This is out of scope for now but the design accommodates it.

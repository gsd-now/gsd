# Mental Model

## Task Queue

The task queue executes arbitrary shell scripts and deserializes their stdout. It is agnostic to what the script does - it could be a simple bash command, a Python script, or an invocation of `gsd_multiplexer submit` to dispatch work to a pool of persistent agents.

### Key Structs

#### 1. Queue Item
The item in the queue. Implements `GsdInProgress<Context>` trait directly.

- Generic over context type
- Associated types:
  - `InProgress` - the in-progress state for this item
  - `Response` - deserialized from script stdout
  - `NextTask: Into<Task>` - constrains which tasks this item can transition to (enables state machine patterns)
- `fn start(self, ctx) -> (Self::InProgress, Command)`:
  - Consumes the queue item and mutable context
  - Returns in-progress state AND a command to run
- `fn cleanup(in_progress, result, ctx) -> Vec<Self::NextTask>`:
  - Called after script completes
  - Returns tasks to add back to the queue (converted via `Into<Task>`)

#### 2. Task Enum
The global task enum. Each variant wraps a queue item. Derives `GsdTask`, which generates dispatch logic to the underlying item's `GsdInProgress` impl.

#### 3. Response Struct
Deserialized from the script's stdout.

### Context

Mutable state passed through the entire processing pipeline. Passed to both `start` and `cleanup`.

### Overall Flow

1. Initialize `queue` with initial items, `in_flight` as empty
2. While `queue` is not empty OR `in_flight` is not empty:
   1. While `in_flight.len() < max_concurrency` AND `queue` is not empty:
      1. Pop item from queue
      2. Call `item.start(ctx)` (synchronous)
      3. Spawn script, add `(in_progress, handle)` to `in_flight`
   2. Wait for any script in `in_flight` to complete
   3. Deserialize stdout into response
   4. Call `cleanup(in_progress, response, ctx)` (synchronous)
   5. Add returned tasks to queue

Only script execution is async/parallel. Everything else (`start`, `cleanup`, queue mutations) is synchronous.

### Processing an Individual Item

1. `item.start(&mut ctx)` returns `(in_progress, command)`
2. Run script (this is the parallel part)
3. Deserialize stdout into `Result<Response, Error>`
4. `Item::cleanup(in_progress, result, &mut ctx)` returns `Vec<NextTask>`
5. Convert each `NextTask` into `Task` via `Into`, add to queue

### Configuration

- Max concurrency: configured on `process_queue`

---

## Multiplexer

The multiplexer (`gsd_multiplexer`) manages a pool of persistent worker agents. It is a CLI with two modes:

### CLI Modes

#### `gsd_multiplexer daemon`
Runs as a daemon, watching folders and dispatching tasks to available agents.

#### `gsd_multiplexer submit <folder> <input>`
Submits a task and blocks until the result is ready:
- Generates random hash ID
- Writes task to `{folder}/{hash}.input`
- Blocks waiting for `{folder}/{hash}.output` to appear
- Prints content of output file to stdout

### Folder Structure

Given a root folder, the structure is:
```
<root>/
├── daemon.lock      # Lock file (PID of running daemon)
├── tasks/           # submit writes {hash}.input, daemon writes {hash}.output
└── agents/
    └── {agent_id}/  # Each agent has its own folder
```

### Protocol
1. Task arrives: `tasks/{hash}.input`
2. Daemon finds available agent
3. Writes to `agents/{agent_id}/input`
4. Agent waits for input file, reads it, deletes it (signals pickup)
5. Agent does work
6. Agent writes `agents/{agent_id}/output`
7. Daemon sees output, copies to `tasks/{hash}.output`, deletes agent output
8. Submit command unblocks, prints output

### Agent Lifecycle
- One task at a time per agent
- Loop: wait for `input` → read → delete → work → write `output` → repeat
- Input deletion signals "picked up"
- Output appearance signals "done"

### Open Questions

- Timeout handling for agents that die or hang
- Agent restart/recovery

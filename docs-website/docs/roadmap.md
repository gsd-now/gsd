# Roadmap

This page documents planned future work for GSD.

## User-Facing Features

### TypeScript Response Schemas

Describe agent return types using TypeScript instead of JSON Schema. TypeScript provides better ergonomics for defining complex types and is more familiar to most developers.

### Reusable Workflow Blocks

Extract workflows as reusable, composable blocks. Configs can reference other config files and inherit/override steps, enabling library-like workflow composition.

### Sequence Primitives

Define sequences of steps as a first-class primitive, making `finally` less special. A step's `next` field points to another step that runs after the subtree completes, with proper value flow between sequence items.

### Multi-Pool Task Routing

Route different steps to different agent pools. Enables heterogeneous workflows where AI agents handle reasoning while a command pool handles shell execution.

### Streaming Responses

Stream responses from tasks instead of waiting for complete JSON objects. Useful for long-running tasks that produce incremental output.

### Structured Nested Results

Get nested results from child tasks in a structured manner. When a task spawns children, collect their results into a structured response for fan-in/reduce patterns.

### Step Prioritization

Priority weights on steps affecting dispatch order. Higher priority tasks get processed first when multiple are queued.

### Runtime Workflow Graph

Visualize workflow execution in real-time, similar to Temporal's UI. Shows which steps ran, parent-child relationships, timing, and agent assignments.

### Claude CLI Integration

Invoke Claude directly from GSD using the Claude CLI, without requiring a separate agent pool setup.

---

## Agent Pool Features

### Socket-Based Agent Protocol

Replace file-based IPC with sockets for faster, more efficient communication. Daemon pushes tasks to connected agents via socket; agents respond over socket.

### Full Socket-Based Protocol

Remove filesystem-based IPC entirely. All communication via socket—no temp files, no directory watching.

---

## Internal Improvements

### task_queue Integration

Make GSD use `task_queue`'s execution engine internally. Unifies the execution model and enables async future-based execution.

### Event-Driven State Persistence

Refactor GSD to operate off a log of events. The persisted log is guaranteed to exactly reproduce the correct state on replay, enabling reliable resume after crashes.

### String Interning

Intern strings (StepName, HookScript, etc.) to reduce memory usage and enable cheap equality checks.

### CLI Version Enforcement

Verify that the agent_pool CLI binary version matches the expected version. Prevents subtle bugs from version mismatches.

### Multi-Threaded Tests

Re-enable parallel test execution. Currently tests run with `--test-threads=1` due to CLI spawn overhead.

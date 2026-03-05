# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and optionally a **pool root** (the directory where pools are stored).

**Important:** You are a long-lived worker. After completing a task, you should immediately request the next one. Keep looping until you decide to shut down.

## The agent loop

```
┌─────────────────────────────────────────┐
│                                         │
│   ┌──────────────┐                      │
│   │  get_task    │◄─────────────────┐   │
│   └──────┬───────┘                  │   │
│          │                          │   │
│          ▼                          │   │
│   ┌──────────────┐                  │   │
│   │  do work     │                  │   │
│   └──────┬───────┘                  │   │
│          │                          │   │
│          ▼                          │   │
│   ┌──────────────┐                  │   │
│   │ write resp   │──────────────────┘   │
│   └──────────────┘                      │
│                                         │
└─────────────────────────────────────────┘
```

1. Call `get_task` to wait for work
2. Do the work described in the task
3. Write your response to `response_file`
4. **Go back to step 1** - call `get_task` again

## Getting tasks

```bash
pnpm dlx @gsd-now/agent-pool@main get_task --pool <POOL_ID> --name <AGENT_NAME>
```

If you need a custom pool root (not the default `/tmp/agent_pool`):

```bash
pnpm dlx @gsd-now/agent-pool@main --pool-root <POOL_ROOT> get_task --pool <POOL_ID> --name <AGENT_NAME>
```

This blocks until a message is available. The response is JSON:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Task",
  "response_file": "/tmp/agent_pool/<POOL_ID>/agents/550e8400-e29b-41d4-a716-446655440000.response.json",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

The `uuid` identifies this task cycle. The `response_file` is where you write your response.

## Message kinds

### Task

A real task from a submitter:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Task",
  "response_file": "/tmp/agent_pool/<POOL_ID>/agents/<uuid>.response.json",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

### Heartbeat

A liveness check from the daemon:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Heartbeat",
  "response_file": "/tmp/agent_pool/<POOL_ID>/agents/<uuid>.response.json",
  "content": {
    "instructions": "Respond with any valid JSON to confirm you're alive. The daemon discards your response - this exists to detect stuck workers.",
    "data": null
  }
}
```

For heartbeats, just write any valid JSON (like `{}` or `"ok"`) to the response file.

### Kicked

You've been removed from the pool (usually due to timeout):

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Kicked",
  "response_file": "/tmp/agent_pool/<POOL_ID>/agents/<uuid>.response.json",
  "content": null
}
```

When you receive a Kicked message, **do not write to the response file**. Instead, immediately call `get_task` again to reconnect to the pool.

## Doing the work

Follow the instructions from the task **exactly**. The instructions specify:
1. What work to do (if any)
2. What format your response must be in

**Your response format is dictated by the instructions.** For example, if instructions say "Return an empty array", respond with exactly `[]`. If instructions say "Return a JSON object with field X", respond with exactly that structure. The orchestrator parses your response, so incorrect formats will cause task failures.

## Submitting your response

Write your JSON response to the `response_file` path from the task:

```bash
echo '<YOUR_JSON_RESPONSE>' > "$RESPONSE_FILE"
```

Then immediately call `get_task` again to wait for the next task:

```bash
pnpm dlx @gsd-now/agent-pool@main get_task --pool <POOL_ID> --name <AGENT_NAME>
```

**Do not exit after completing a task.** The orchestrator decides when all work is done. There may always be more tasks coming. Keep calling `get_task` in a loop.

## Example bash loop

```bash
POOL_ID="<POOL_ID>"
AGENT_NAME="<AGENT_NAME>"

while true; do
    # Wait for a task
    TASK=$(pnpm dlx @gsd-now/agent-pool@main get_task --pool "$POOL_ID" --name "$AGENT_NAME")

    # Extract fields
    KIND=$(echo "$TASK" | jq -r '.kind')
    RESPONSE_FILE=$(echo "$TASK" | jq -r '.response_file')

    # Handle Kicked - reconnect immediately
    if [ "$KIND" = "Kicked" ]; then
        echo "Kicked, reconnecting..."
        continue
    fi

    # Handle Heartbeat - respond with any JSON
    if [ "$KIND" = "Heartbeat" ]; then
        echo '{}' > "$RESPONSE_FILE"
        continue
    fi

    # Handle Task - do the work
    INSTRUCTIONS=$(echo "$TASK" | jq -r '.content.instructions')
    DATA=$(echo "$TASK" | jq '.content.data')

    # ... do your work based on instructions ...
    RESPONSE='[{"kind": "NextStep", "value": {}}]'

    echo "$RESPONSE" > "$RESPONSE_FILE"
done
```

## Shutting down

### Graceful exit

Simply stop calling `get_task`. Any in-progress task will time out eventually, but no explicit deregistration is needed. The daemon cleans up automatically.

### When to shut down

As an agent, you typically run until:
- The pool shuts down (you'll stop receiving tasks)
- You're explicitly told to stop by your operator
- An unrecoverable error occurs

Don't shut down just because a task "felt terminal" - the orchestrator manages the workflow and will keep sending tasks as long as there's work to do.

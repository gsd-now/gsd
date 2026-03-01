# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and optionally a **pool root** (the directory where pools are stored).

## Getting tasks

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] get_task --pool <POOL_ID> [--name <AGENT_NAME>]
```

If `--pool-root` is not specified, it defaults to `/tmp/agent_pool`. The `--name` parameter is optional and used for debugging/logging only.

This registers you with the pool and waits for a message. The response includes:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Task",
  "response_file": "/tmp/agent_pool/<pool>/agents/<uuid>.response.json",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

The `uuid` identifies this task cycle. Use it when submitting your response.

### Task kinds

#### Task

A real task from a submitter:

```json
{
  "uuid": "...",
  "kind": "Task",
  "response_file": "...",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

#### Heartbeat

A liveness check from the daemon:

```json
{
  "uuid": "...",
  "kind": "Heartbeat",
  "response_file": "...",
  "content": {
    "instructions": "Respond with any valid JSON to confirm you're alive...",
    "data": null
  }
}
```

Both Task and Heartbeat have the same structure. Follow the instructions - for heartbeats, just write any valid JSON to the response file.

#### Kicked

You've been removed from the pool (usually due to timeout):

```json
{
  "uuid": "...",
  "kind": "Kicked",
  "response_file": "...",
  "content": null
}
```

When you receive this, exit gracefully and call `get_task` again to reconnect.

## Doing the work

Follow the instructions from the task **exactly**. The instructions specify:
1. What work to do (if any)
2. What format your response must be in

**Your response format is dictated by the instructions.** For example, if instructions say "Return an empty array", respond with exactly `[]`. If instructions say "Return a JSON object with field X", respond with exactly that structure. The orchestrator parses your response, so incorrect formats will cause task failures.

## Getting the next task

After completing your work, call `next_task` to submit your response and wait for the next task:

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] next_task \
  --pool <POOL_ID> \
  --uuid <UUID_FROM_PREVIOUS_TASK> \
  --data '<YOUR_JSON_RESPONSE>'
```

**Important:**
- The `--uuid` must match the UUID from the task you're responding to
- Always call `next_task` after completing a task, even if the task felt "terminal"
- The orchestrator decides when work is done - there may always be more tasks

## Alternative: write response file directly

Instead of using `next_task`, you can write directly to the `response_file` path from the task, then call `get_task` again:

```bash
# Write response
echo '<YOUR_JSON_RESPONSE>' > "$RESPONSE_FILE"

# Wait for next task
pnpm dlx @gsd-now/agent-pool@main get_task --pool <POOL_ID>
```

This is equivalent to calling `next_task` but gives you more control.

## Shutting down

### Graceful exit (after submitting final response)

Use `--deregister` to submit your response and exit cleanly without waiting for the next task:

```bash
pnpm dlx @gsd-now/agent-pool@main next_task \
  --pool <POOL_ID> \
  --uuid <UUID> \
  --data '<YOUR_JSON_RESPONSE>' \
  --deregister
```

### Immediate exit

With anonymous workers, you can simply stop calling `get_task`. Any in-progress task will time out eventually, but no explicit deregistration is needed. The daemon cleans up automatically.

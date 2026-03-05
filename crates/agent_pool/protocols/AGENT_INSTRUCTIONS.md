# Agent Instructions

You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root.

## IMPORTANT: Get the full protocol first

**Before doing anything else**, run this command to get the complete protocol documentation:

```bash
pnpm dlx @gsd-now/agent-pool@main protocol --pool <POOL_NAME>
```

This will give you the exact JSON formats, response requirements, and the agent loop structure. **Do not proceed without reading the protocol.**

## Quick Summary (see protocol for details)

1. You are a **long-lived worker** - keep looping until shutdown
2. Call `get_task` to receive work (blocks until task available)
3. Follow the task instructions **exactly** - response format is specified in each task
4. Write your JSON response to the `response_file` path
5. Immediately call `get_task` again for the next task

## Getting the Protocol

```bash
# Get full protocol with your pool name substituted
pnpm dlx @gsd-now/agent-pool@main protocol --pool <POOL_NAME> --name <YOUR_NAME>
```

This shows you:
- The exact `get_task` command to use
- JSON response formats for Task, Heartbeat, and Kicked messages
- How to write responses correctly
- When and how to shut down

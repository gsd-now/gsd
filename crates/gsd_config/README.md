# gsd_config

Declarative task orchestrator that sits on top of `agent_pool`.

## Overview

Define task state machines via declarative config:
- Validates tasks against JSON schemas at runtime
- Generates markdown documentation for agents
- Handles timeouts and retries with per-step options

The config format is serialization-agnostic (uses serde). The CLI handles parsing from JSON (other formats could be added).

## Config Format

```json
{
  "options": {
    "timeout": 120,
    "max_retries": 3,
    "retry_on_timeout": true,
    "retry_on_invalid_response": false
  },
  "steps": [
    {
      "name": "Analyze",
      "schema": { "kind": "Inline", "value": { "type": "object" } },
      "instructions": "Analyze the given file.",
      "next": ["Implement", "Done"]
    },
    {
      "name": "Implement",
      "instructions": "Implement the changes.",
      "next": ["Test"],
      "options": {
        "timeout": 300,
        "max_retries": 5,
        "retry_on_timeout": true,
        "retry_on_invalid_response": false
      }
    },
    {
      "name": "Test",
      "instructions": "Run tests.",
      "next": ["Done", "Implement"]
    },
    {
      "name": "Done",
      "next": []
    }
  ]
}
```

### Global Options

- `timeout`: Seconds before a task times out (default: none)
- `max_retries`: Max retry attempts (default: 0)
- `retry_on_timeout`: Whether to retry timed-out tasks (default: true)
- `retry_on_invalid_response`: Whether to retry invalid responses (default: true)

### Step Fields

- `name`: Step identifier
- `schema`: JSON Schema for validation (optional)
  - `null` or omitted → accepts any value
  - `{ "kind": "Inline", "value": {...} }` → inline schema
  - `{ "kind": "Link", "value": "path/to/schema.json" }` → external file
- `instructions`: Markdown shown to agents
- `next`: Valid next step names (empty = terminal)
- `options`: Per-step overrides for global options

## Task Format

Tasks are JSON objects with `kind` and `value`:

```json
{"kind": "Analyze", "value": {"file": "src/main.rs"}}
```

Agent responses are arrays of tasks:

```json
[{"kind": "Implement", "value": {"changes": "..."}}]
```

## CLI Usage

```bash
# Run with config file
gsd run config.json --initial '[{"kind": "Analyze", "value": {}}]'

# Run with inline config (from script)
gsd run '{"steps": [...]}' --initial tasks.json

# With agent_pool root and wake script
gsd run config.json --root /tmp/pool --wake ./wake.sh --initial tasks.json

# Log to file for later analysis
gsd run config.json --log-file run.log --initial tasks.json

# Generate docs
gsd docs config.json

# Validate config
gsd validate config.json
```

## Timeout Behavior

1. Task dispatched to agent
2. If no response within `timeout` seconds:
   - Submit cancelled
   - Task requeued if `retry_on_timeout` is true and `max_retries` not exceeded

## Runtime Validation

1. Incoming task validated against step's schema
2. Agent response validated:
   - Must be JSON array
   - Each item's `kind` must be a valid `next` step
   - Each item's `value` validated against target step's schema
3. Invalid responses requeued if `retry_on_invalid_response` is true

## Agent Documentation

GSD auto-generates markdown for agents. The payload sent to agents includes:

```json
{
  "task": {"kind": "Analyze", "value": {...}},
  "instructions": "# Current Step: Analyze\n\n...",
  "timeout_seconds": 120
}
```

The instructions include:
- Step name and instructions
- Valid next steps with schema info
- Example response format

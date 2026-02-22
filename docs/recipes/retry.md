# Retry Policies

Configure how GSD handles failures and retries.

## Global Options

```json
{
  "options": {
    "timeout": 120,
    "max_retries": 3,
    "retry_on_timeout": true,
    "retry_on_invalid_response": true
  },
  "steps": [...]
}
```

## Per-Step Overrides

Override global settings for specific steps:

```json
{
  "options": {
    "timeout": 60,
    "max_retries": 2
  },
  "steps": [
    {
      "name": "QuickCheck",
      "options": {
        "timeout": 10,
        "max_retries": 0
      },
      "action": { "kind": "Pool", "instructions": "..." },
      "next": []
    },
    {
      "name": "ExpensiveAnalysis",
      "options": {
        "timeout": 300,
        "max_retries": 5
      },
      "action": { "kind": "Pool", "instructions": "..." },
      "next": []
    }
  ]
}
```

## Retry Triggers

| Condition | Option | Default |
|-----------|--------|---------|
| Agent times out | `retry_on_timeout` | `true` |
| Invalid JSON response | `retry_on_invalid_response` | `true` |
| Invalid transition (wrong `kind`) | `retry_on_invalid_response` | `true` |
| Schema validation failure | `retry_on_invalid_response` | `true` |
| Submit error (network, etc.) | Always retried | - |

## Disabling Retries

For idempotent-sensitive operations:

```json
{
  "steps": [
    {
      "name": "SendEmail",
      "options": {
        "retry_on_timeout": false,
        "retry_on_invalid_response": false,
        "max_retries": 0
      },
      "action": { "kind": "Pool", "instructions": "..." },
      "next": []
    }
  ]
}
```

## Task Outcomes

After processing, each task has one of these outcomes:

- **Completed**: Agent returned valid response, new tasks spawned
- **Requeued**: Failure occurred, task will be retried
- **Dropped**: Max retries exceeded or retry disabled
- **Skipped**: Initial validation failed (unknown step, invalid schema)

## Key Points

- Retries increment a counter on the task (`task.retries`)
- Tasks are requeued at the back of the queue
- Use `max_retries: 0` to never retry (fail fast)
- Combine with schema validation to catch bad responses early

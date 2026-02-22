# Branching

Branching allows agents to choose different paths based on their analysis.

## Example: Approval Workflow

```json
{
  "steps": [
    {
      "name": "Review",
      "action": {
        "kind": "Pool",
        "instructions": "Review this PR. If it looks good, return `[{\"kind\": \"Approve\", \"value\": {...}}]`. If changes are needed, return `[{\"kind\": \"RequestChanges\", \"value\": {...}}]`."
      },
      "next": ["Approve", "RequestChanges"]
    },
    {
      "name": "Approve",
      "action": {
        "kind": "Pool",
        "instructions": "Merge the PR. Return `[]`."
      },
      "next": []
    },
    {
      "name": "RequestChanges",
      "action": {
        "kind": "Pool",
        "instructions": "Comment on the PR with requested changes. Return `[]`."
      },
      "next": []
    }
  ]
}
```

## Flow

```
        ┌─→ Approve → (done)
Review ─┤
        └─→ RequestChanges → (done)
```

## Key Points

- The `next` array lists ALL valid transitions from a step
- The agent's response determines which path is taken
- Agents can only transition to steps listed in `next`
- Invalid transitions cause retries (configurable)

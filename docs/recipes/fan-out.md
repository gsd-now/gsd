# Fan-Out

Fan-out splits one task into multiple parallel tasks.

## Example: Parallel File Processing

```json
{
  "steps": [
    {
      "name": "ListFiles",
      "action": {
        "kind": "Command",
        "script": "find src -name '*.rs' | jq -R -s 'split(\"\\n\") | map(select(length > 0)) | map({kind: \"ProcessFile\", value: {path: .}})'"
      },
      "next": ["ProcessFile"]
    },
    {
      "name": "ProcessFile",
      "action": {
        "kind": "Pool",
        "instructions": "Analyze this file. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```

## Flow

```
              ┌─→ ProcessFile (file1.rs)
              │
ListFiles ────┼─→ ProcessFile (file2.rs)
              │
              └─→ ProcessFile (file3.rs)
```

## Agent Fan-Out

Agents can also fan out by returning multiple tasks:

```json
{
  "steps": [
    {
      "name": "Analyze",
      "action": {
        "kind": "Pool",
        "instructions": "Find all functions that need refactoring. Return one task per function: `[{\"kind\": \"Refactor\", \"value\": {\"function\": \"...\"}}, ...]`"
      },
      "next": ["Refactor"]
    },
    {
      "name": "Refactor",
      "action": {
        "kind": "Pool",
        "instructions": "Refactor this function. Return `[]`."
      },
      "next": []
    }
  ]
}
```

## Key Points

- Return an array with multiple tasks to fan out
- All fanned-out tasks run in parallel (up to `max_concurrency`)
- Each task is independent - failures don't affect siblings

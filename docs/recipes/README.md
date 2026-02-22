# GSD Recipes

This directory contains example configurations for common task queue patterns.

## Quick Reference

| Recipe | Description |
|--------|-------------|
| [Linear Pipeline](linear-pipeline.md) | Step-by-step processing (A → B → C) |
| [Branching](branching.md) | Conditional paths based on output |
| [Fan-Out](fan-out.md) | Split one task into many parallel tasks |
| [Fan-In](fan-in.md) | Collect results from multiple tasks |
| [Fan-Out with Finally](fan-out-finally.md) | Fan-out → aggregate → continue |
| [Pre/Post/Finally Hooks](hooks.md) | Transform data, aggregate results, cleanup |
| [Validation](validation.md) | Schema validation for inputs and outputs |
| [Retry Policies](retry.md) | Handle failures gracefully |
| [Local Commands](commands.md) | Run shell scripts instead of agents |
| [Nested GSD](nested-gsd.md) | Launch sub-workflows from commands |

## Config Structure

Every GSD config has this structure:

```json
{
  "options": {
    "timeout": 60,
    "max_retries": 3,
    "max_concurrency": 5
  },
  "steps": [
    {
      "name": "StepName",
      "value_schema": { ... },
      "pre": "optional-pre-hook.sh",
      "action": { "kind": "Pool", "instructions": "..." },
      "post": "optional-post-hook.sh",
      "finally": "optional-finally-hook.sh",
      "next": ["NextStep1", "NextStep2"]
    }
  ]
}
```

## Terminology

- **Step**: A named stage in your workflow (e.g., "Analyze", "Implement", "Review")
- **Task**: An instance of a step with a specific value (e.g., `{step: "Analyze", value: {file: "main.rs"}}`)
- **Action**: What happens when a task runs (Pool = send to agent, Command = run locally)
- **Transition**: Moving from one step to another via the `next` field
- **Hook**: A shell command that runs before (pre), after (post), or when descendants complete (finally)

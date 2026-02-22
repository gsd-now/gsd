# Schema Validation

Use JSON Schema to validate task inputs and ensure agents return valid transitions.

## Input Validation

Validate the value payload for each step:

```json
{
  "steps": [
    {
      "name": "ProcessOrder",
      "value_schema": {
        "type": "object",
        "required": ["order_id", "items"],
        "properties": {
          "order_id": { "type": "string", "pattern": "^ORD-[0-9]+$" },
          "items": {
            "type": "array",
            "items": {
              "type": "object",
              "required": ["sku", "quantity"],
              "properties": {
                "sku": { "type": "string" },
                "quantity": { "type": "integer", "minimum": 1 }
              }
            }
          }
        }
      },
      "action": { "kind": "Pool", "instructions": "..." },
      "next": ["Ship"]
    }
  ]
}
```

## External Schema Files

Reference schemas from files:

```json
{
  "steps": [
    {
      "name": "ProcessOrder",
      "value_schema": "schemas/order.json",
      "action": { "kind": "Pool", "instructions": "..." },
      "next": ["Ship"]
    }
  ]
}
```

## What Gets Validated

1. **Initial tasks**: Validated against their step's `value_schema`
2. **Agent responses**: Validated to ensure:
   - Response is a JSON array
   - Each task has a valid `kind` (matches a step in `next`)
   - Each task's `value` matches the target step's `value_schema`

## Validation Failures

When validation fails:

- **Initial tasks**: Skipped with a warning
- **Agent responses**: Treated as invalid response, triggers retry policy

## Key Points

- Schemas are optional (omit `value_schema` to accept any value)
- Use `retry_on_invalid_response: false` to drop tasks instead of retrying
- Schemas help catch agent mistakes early

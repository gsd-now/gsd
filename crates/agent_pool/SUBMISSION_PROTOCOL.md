# Task Submission

Submit tasks to an agent pool and wait for results.

## Usage

**From a file (works in sandboxed environments):**
```bash
agent_pool submit_task --pool <POOL_ID> --file /path/to/task.json
```

**Inline JSON (faster, requires socket access):**
```bash
agent_pool submit_task --pool <POOL_ID> --input '{"task":...}'
```

Both methods block until the task completes (default timeout: 5 minutes).

## When to use which

- Use `--file` in sandboxed environments where Unix sockets are blocked
- Use `--input` for quick submissions when you have full system access

The output is the same either way: JSON response on stdout, errors on stderr.

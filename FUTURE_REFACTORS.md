# Future Refactors

Ideas and improvements to implement later.

## Command Agent Improvements

### Reconnect on Timeout

When the command agent is kicked due to heartbeat timeout, it should automatically reconnect instead of exiting. Currently if the agent is slow to respond to a heartbeat (e.g., because it's running a long command), it gets kicked and the agent script exits.

**Current behavior:**
- Agent receives Kicked message → exits
- User must manually restart the agent

**Desired behavior:**
- Agent receives Kicked message → re-registers with same name
- Seamlessly continues processing tasks

### Command Timeout

The command agent should have its own configurable timeout for executing commands, separate from the daemon's heartbeat timeout.

**Problem:**
- Daemon heartbeat timeout is ~60s
- Some commands take longer than that
- Agent gets kicked while command is still running

**Solution:**
- Add `--timeout` flag to command agent
- Execute commands with timeout wrapper
- If command times out, return error response instead of hanging

## GSD Configuration

### Default Step

Add the ability to mark one step as the "default" entry point, so users don't need to specify `--initial` with the full task structure.

**Current:**
```bash
gsd run config.json --pool p1 --initial '[{"kind": "Analyze", "value": {"file_url": "/path/to/file"}}]'
```

**Desired:**
```bash
gsd run config.json --pool p1 --file /path/to/file
```

The config would specify which step is the default and how CLI args map to the value schema.

**Possible syntax:**
```json
{
  "default_step": "Analyze",
  "cli_mapping": {
    "file": "file_url"
  }
}
```

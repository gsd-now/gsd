# Health Check Plan

Task-based health checks (ping-pong) to verify agent health and pre-approve tool use.

## Motivation

### Initial Health Check

When an agent first connects, we send a health check that forces it through the full protocol:
1. Agent receives task
2. Agent writes response
3. Daemon confirms receipt

**Why this matters:** For AI agents that require human approval for tool use, the initial health check gets approval out of the way with a harmless dummy task. After that, subsequent real tasks follow the same pattern, so they don't block on human interaction.

### Periodic Health Check

Periodically send health checks to **idle** agents to verify they're still alive and responsive. This only happens when an agent has no task in progress - the purpose is to:
1. Keep agents from sleeping/timing out
2. Detect agents that have disconnected

If an agent fails to respond within the timeout, we deregister it from the pool.

## Agent Status Enum

The daemon tracks each agent's status with a clear enum:

```rust
enum AgentStatus {
    /// Agent is idle, available for work
    Idle,
    /// Agent is busy with something
    Busy(InFlight),
}

enum InFlight {
    /// Working on a real task from a submitter
    Task { respond_to: ResponseTarget },
    /// Responding to a health check
    HealthCheck,
}
```

State transitions:

```
                    ┌─────────────────────────────────┐
                    │                                 │
                    ▼                                 │
Agent registers ──► Busy(HealthCheck) ──response──► Idle
                                      ──timeout───► [removed]

                    ┌─────────────────────────────────┐
                    │                                 │
                    ▼                                 │
Timer fires ──────► Busy(HealthCheck) ──response──► Idle
(idle agent)                          ──timeout───► [removed]

                    ┌─────────────────────────────────┐
                    │                                 │
                    ▼                                 │
Task dispatched ──► Busy(Task{...}) ───response───► Idle
```

No distinction between initial and periodic health checks - both result in `Busy(HealthCheck)`.

## Configuration

```rust
pub struct DaemonConfig {
    /// Send a health check when agent first registers.
    /// Helps get tool-use approvals out of the way.
    /// Default: true
    pub initial_health_check: bool,

    /// Send periodic health checks to idle agents.
    /// Default: true
    pub periodic_health_check: bool,

    /// Interval between periodic health checks for idle agents.
    /// Default: 60 seconds
    pub health_check_interval: Duration,

    /// How long to wait for health check response before deregistering agent.
    /// Default: 30 seconds
    pub health_check_timeout: Duration,
}
```

## Protocol

### Task Format

A health check uses the outer `kind` field to distinguish it from real tasks:

**Health check:**
```json
{
  "kind": "HealthCheck",
  "response_file": "/tmp/gsd/abc123/agents/claude-1/response.json",
  "content": {
    "instructions": "Respond with any value to confirm you are alive."
  }
}
```

**Real task:**
```json
{
  "kind": "Task",
  "response_file": "/tmp/gsd/abc123/agents/claude-1/response.json",
  "content": {
    "task": {"name": "Analyze", "value": {"files": ["main.rs"]}},
    "instructions": "Analyze these files..."
  }
}
```

The outer `kind` is statically known: `Task` or `HealthCheck`. The step name (`Analyze`) only appears inside real tasks. No collision possible with user-defined step names.

### Response

Any response works. The daemon just checks that a response file appeared.

```json
{}
```

### Agent Handling

Agents check the outer `kind`:

```bash
KIND=$(echo "$TASK" | jq -r '.kind')
if [ "$KIND" = "HealthCheck" ]; then
    echo "{}" > "$RESPONSE_FILE"
else
    # Handle real task...
fi
```

## Daemon Flow

### On Agent Registration

```
Agent registers (creates directory)
    │
    ▼
Daemon detects new agent
    │
    ▼
if initial_health_check:
    │
    ├─► status = Busy(HealthCheck)
    │   dispatch health check task
    │       │
    │       ├─ Response: status = Idle
    │       │
    │       └─ Timeout: remove agent
    │
else:
    │
    └─► status = Idle
```

### Periodic Health Check

```
Timer fires (every health_check_interval)
    │
    ▼
For each agent where status == Idle:
    │
    ▼
status = Busy(HealthCheck)
dispatch health check task
    │
    ├─ Response: status = Idle, reset timer
    │
    └─ Timeout: remove agent
```

### Recovery After Timeout

When an agent times out, it's removed from the pool. If the agent is still alive and calls `get_task`, it re-registers and goes through the initial health check again. Automatic recovery - no manual intervention needed.

## Implementation Tasks

### Task 1: Add AgentStatus enum

**Files:** `crates/agent_pool/src/daemon.rs`

Replace the current `Option<InFlightTask>` with proper enums:

```rust
enum AgentStatus {
    Idle,
    Busy(InFlight),
}

enum InFlight {
    Task { respond_to: ResponseTarget },
    HealthCheck,
}

struct AgentState {
    status: AgentStatus,
    last_activity: Instant,
}
```

**Commit:** `refactor(agent_pool): add AgentStatus enum for clear state modeling`

---

### Task 2: Update protocol for HealthCheck kind

**Files:**
- `crates/agent_pool/src/main.rs` (get_task output format)
- `crates/agent_pool/AGENT_PROTOCOL.md`

Change outer `kind` to be `Task` or `HealthCheck` instead of always `Task`.

**Commit:** `feat(agent_pool): add HealthCheck as distinct task kind`

---

### Task 3: Add health check config options

**Files:** `crates/agent_pool/src/daemon.rs`

```rust
pub struct DaemonConfig {
    pub initial_health_check: bool,       // default: true
    pub periodic_health_check: bool,      // default: true
    pub health_check_interval: Duration,  // default: 60s
    pub health_check_timeout: Duration,   // default: 30s
}
```

**Commit:** `feat(agent_pool): add health check config options`

---

### Task 4: Add CLI flags for health check config

**Files:** `crates/agent_pool/src/main.rs`

```rust
Command::Start {
    #[arg(long, default_value = "true")]
    initial_health_check: bool,
    #[arg(long, default_value = "true")]
    periodic_health_check: bool,
    #[arg(long, default_value = "60")]
    health_check_interval_secs: u64,
    #[arg(long, default_value = "30")]
    health_check_timeout_secs: u64,
}
```

**Commit:** `feat(agent_pool): add health check CLI flags`

---

### Task 5: Implement health check dispatch

**Files:** `crates/agent_pool/src/daemon.rs`

- On registration (if enabled): set status to `Busy(HealthCheck)`, dispatch
- On timer (for idle agents): set status to `Busy(HealthCheck)`, dispatch

**Commit:** `feat(agent_pool): implement health check dispatch`

---

### Task 6: Handle health check response and timeout

**Files:** `crates/agent_pool/src/daemon.rs`

- Response received while `Busy(HealthCheck)`: set status to `Idle`
- Timeout while `Busy(HealthCheck)`: remove agent directory and state

**Commit:** `feat(agent_pool): handle health check response and timeout`

---

### Task 7: Update scripts to handle health checks

**Files:**
- `crates/agent_pool/scripts/command-agent.sh`
- `crates/agent_pool/scripts/echo-agent.sh`

```bash
KIND=$(echo "$TASK_JSON" | jq -r '.kind')
if [ "$KIND" = "HealthCheck" ]; then
    echo "[$NAME] Responding to health check" >&2
    echo "{}" > "$RESPONSE_FILE"
    continue
fi
```

**Commit:** `feat(agent_pool): handle health checks in shell agent scripts`

---

### Task 8: Update demos

**Files:** `crates/agent_pool/demos/*.sh`

Either handle health checks or disable them:

```bash
agent_pool start --pool "$POOL" --initial-health-check=false --periodic-health-check=false &
```

**Commit:** `feat(agent_pool): update demos for health checks`

---

### Task 9: Add tests

**Files:** `crates/agent_pool/tests/health_check.rs`

Test cases:
- Initial health check sent on registration (when enabled)
- Agent status is `Busy(HealthCheck)` until response
- Periodic health check sent after interval (to idle agents only)
- Agent removed on health check timeout
- Agent can re-register after timeout
- Health checks disabled via config

**Commit:** `test(agent_pool): add health check tests`

---

## Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1 | Add AgentStatus enum | - |
| 2 | Update protocol for HealthCheck kind | - |
| 3 | Add config options | - |
| 4 | Add CLI flags | 3 |
| 5 | Implement dispatch | 1, 3 |
| 6 | Handle response/timeout | 1, 5 |
| 7 | Update shell scripts | 2 |
| 8 | Update demos | 4, 7 |
| 9 | Add tests | 5, 6 |

Tasks 1, 2, 3 can be done in parallel.

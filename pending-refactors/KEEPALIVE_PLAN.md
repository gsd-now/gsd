# Keepalive Plan

Replace file-based heartbeats with task-based keepalives (ping-pong).

## Motivation

### Initial Keepalive

When an agent (like Claude) first connects, we send a dummy "ping" task that forces it through the full protocol:
1. Agent receives task
2. Agent writes response
3. Daemon confirms receipt

**Why this matters:** For AI agents that require human approval for tool use, the initial keepalive gets approval out of the way with a harmless dummy task. After that, subsequent real tasks have already been "approved" by the same pattern, so they don't block on human interaction.

### Periodic Keepalive

Periodically send ping tasks to **idle** agents to verify they're still alive and responsive. This only happens when an agent has no task in progress - the purpose is to keep agents from sleeping and to detect agents that have disconnected.

**Key distinction from heartbeats:** Keepalives are sent *to* idle agents, not *from* busy agents. There is no in-flight work to worry about during a keepalive - if an agent fails to respond, we simply deregister it.

If an agent fails to respond within the timeout:
1. Log a warning
2. Remove the agent's directory (deregister)
3. The agent is no longer part of the pool

## Target State

```rust
pub struct DaemonConfig {
    /// Send a ping task when agent first registers.
    /// Helps get tool-use approvals out of the way.
    /// Default: true
    pub initial_keepalive: bool,

    /// Send periodic ping tasks to check agent health.
    /// Default: true
    pub periodic_keepalive: bool,

    /// Interval between periodic keepalives.
    /// Default: 60 seconds
    pub keepalive_interval: Duration,

    /// How long to wait for keepalive response before marking agent dead.
    /// Default: 30 seconds
    pub keepalive_timeout: Duration,
}
```

### Ping Task Format

A keepalive is just a regular task with a special marker:

```json
{
    "task": {
        "kind": "Keepalive",
        "value": { "id": "ping-12345" }
    },
    "instructions": "Respond with the exact same id to confirm you are alive."
}
```

Expected response:
```json
{ "id": "ping-12345" }
```

### Agent Handling

Agents treat keepalives like any other task. The instructions tell them what to do. No special client-side code needed - just follow the instructions.

For the command-agent.sh, we'd add handling:
```bash
KIND=$(echo "$TASK_JSON" | jq -r '.content.task.kind')
if [ "$KIND" = "Keepalive" ]; then
    ID=$(echo "$TASK_JSON" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

### Daemon Flow

**Initial keepalive (on agent registration):**
```
Agent registers
    │
    ▼
Daemon sends Keepalive task
    │
    ▼
Agent responds (or times out)
    │
    ├─ Success: Agent marked as available for real tasks
    │
    └─ Timeout: Agent removed from pool
```

**Periodic keepalive:**
```
Timer fires (every keepalive_interval)
    │
    ▼
For each idle agent:
    │
    ▼
Send Keepalive task
    │
    ▼
Wait for response (up to keepalive_timeout)
    │
    ├─ Success: Agent stays in pool
    │
    └─ Timeout: Agent removed from pool
```

## Implementation Tasks

### Task 1: Add Keepalive task kind to protocol

**Files:** `crates/agent_pool/AGENT_PROTOCOL.md`

Add documentation about Keepalive tasks. No code changes.

**Commit:** `docs: document Keepalive task kind in agent protocol`

---

### Task 2: Add keepalive config options

**Files:** `crates/agent_pool/src/daemon.rs`

```rust
pub struct DaemonConfig {
    pub initial_keepalive: bool,      // default: true
    pub periodic_keepalive: bool,     // default: true
    pub keepalive_interval: Duration, // default: 60s
    pub keepalive_timeout: Duration,  // default: 30s
    // Keep existing heartbeat_timeout for backward compat (deprecated)
    pub heartbeat_timeout: Option<Duration>,
}
```

**Commit:** `feat(agent_pool): add keepalive config options`

---

### Task 3: Add CLI flags for keepalive config

**Files:** `crates/agent_pool/src/main.rs`

```rust
Command::Start {
    #[arg(long, default_value = "true")]
    initial_keepalive: bool,
    #[arg(long, default_value = "true")]
    periodic_keepalive: bool,
    #[arg(long, default_value = "60")]
    keepalive_interval_secs: u64,
    #[arg(long, default_value = "30")]
    keepalive_timeout_secs: u64,
}
```

**Commit:** `feat(agent_pool): add keepalive CLI flags to start command`

---

### Task 4: Implement initial keepalive on registration

**Files:** `crates/agent_pool/src/daemon.rs`

When an agent registers:
1. If `initial_keepalive` is true, mark agent as "pending_keepalive"
2. Dispatch a Keepalive task immediately
3. On response, mark agent as "available"
4. On timeout, remove agent

```rust
fn register(&mut self, agent_id: &str) {
    if self.config.initial_keepalive {
        self.agents.insert(agent_id.to_string(), AgentState::pending_keepalive());
        self.dispatch_keepalive(agent_id);
    } else {
        self.agents.insert(agent_id.to_string(), AgentState::available());
    }
}
```

**Commit:** `feat(agent_pool): send initial keepalive on agent registration`

---

### Task 5: Implement periodic keepalive

**Files:** `crates/agent_pool/src/daemon.rs`

In the event loop, track last keepalive time per agent. When interval elapses for an idle agent, send a keepalive.

```rust
struct AgentState {
    status: AgentStatus,
    last_keepalive: Option<Instant>,
    in_flight: Option<InFlightTask>,
}

fn check_periodic_keepalives(&mut self) {
    if !self.config.periodic_keepalive {
        return;
    }
    for (id, agent) in &mut self.agents {
        if agent.is_idle() && agent.needs_keepalive(self.config.keepalive_interval) {
            self.dispatch_keepalive(id);
        }
    }
}
```

**Commit:** `feat(agent_pool): implement periodic keepalive checks`

---

### Task 6: Handle keepalive timeout

**Files:** `crates/agent_pool/src/daemon.rs`

When a keepalive task times out:
1. Log warning
2. Remove agent directory (deregister)
3. Remove agent from internal state

Since keepalives only happen to idle agents, there's no in-flight work to worry about.

```rust
fn handle_keepalive_timeout(&mut self, agent_id: &str) {
    warn!(agent_id, "keepalive timeout, deregistering agent");

    // Remove the agent directory
    let agent_dir = self.agents_dir.join(agent_id);
    let _ = fs::remove_dir_all(&agent_dir);

    // Remove from internal state
    self.agents.remove(agent_id);
}
```

**Commit:** `feat(agent_pool): deregister agent on keepalive timeout`

---

### Task 7: Update command-agent.sh to handle keepalives

**Files:** `crates/agent_pool/scripts/command-agent.sh`

Add special handling for Keepalive tasks:

```bash
KIND=$(echo "$TASK_JSON" | jq -r '.content.task.kind // empty')
if [ "$KIND" = "Keepalive" ]; then
    echo "[$NAME] Responding to keepalive" >&2
    ID=$(echo "$TASK_JSON" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

**Commit:** `feat(agent_pool): handle keepalive tasks in command-agent.sh`

---

### Task 8: Add tests for keepalive behavior

**Files:** `crates/agent_pool/tests/keepalive.rs`

Test cases:
- Initial keepalive sent on registration
- Agent not available until keepalive response
- Periodic keepalive sent after interval
- Agent removed on keepalive timeout
- In-flight task requeued on timeout

**Commit:** `test(agent_pool): add keepalive behavior tests`

---

## Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1 | Document Keepalive in protocol | - |
| 2 | Add config options | - |
| 3 | Add CLI flags | 2 |
| 4 | Initial keepalive | 2 |
| 5 | Periodic keepalive | 2 |
| 6 | Timeout handling | 4, 5 |
| 7 | Update command-agent.sh | 1 |
| 8 | Add tests | 4, 5, 6 |

Tasks 1, 2, 7 can be done in parallel. Tasks 4, 5 can be done in parallel after 2. The goal is small, atomic commits that each leave the system in a working state.

**Note:** Heartbeat mechanism was already removed in a prior commit.

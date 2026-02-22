# Health Check Plan

Task-based health checks (ping-pong) to verify agent health and pre-approve tool use.

## Motivation

### Initial Health Check

When an agent first connects, we send a health check that forces it through the full protocol. This gets tool-use approvals out of the way with a harmless dummy task.

### Periodic Health Check

Periodically send health checks to **idle** agents to verify they're still alive. If an agent fails to respond within the timeout, we deregister it. Agents can recover by calling `get_task` again.

---

## State Machine

### Agent States

```
                    ┌─────────────────────────────────────┐
                    │                                     │
                    ▼                                     │
    ┌───────────────────────────────┐                    │
    │             Idle              │                    │
    │  (available for work)         │                    │
    └───────────────────────────────┘                    │
           │                │                            │
           │ dispatch_to()  │ dispatch_health_check()    │
           │                │                            │
           ▼                ▼                            │
    ┌──────────────┐  ┌──────────────┐                   │
    │ Busy(Task)   │  │ Busy(Health) │                   │
    │              │  │              │                   │
    └──────────────┘  └──────────────┘                   │
           │                │         │                  │
           │ complete()     │         │ timeout          │
           │                │         │                  │
           ▼                ▼         ▼                  │
    ┌──────────────┐  ┌──────────────┐  ┌──────────────┐ │
    │ send_response│  │ log success  │  │ deregister   │ │
    │ to submitter │  │              │  │ agent        │ │
    └──────────────┘  └──────────────┘  └──────────────┘ │
           │                │                            │
           └────────────────┴────────────────────────────┘
```

### Typestate Guarantee

The `InFlight` enum ensures exhaustive handling. Adding `HealthCheck` variant forces us to handle it in exactly one place:

```rust
impl InFlight {
    /// Consumes the in-flight work and performs completion action.
    /// Exhaustive match guarantees all variants are handled.
    fn complete(self, output: String) -> io::Result<()> {
        match self {
            InFlight::Task { respond_to } => {
                // MUST send response to submitter
                send_response(respond_to, &Response::processed(output))
            }
            InFlight::HealthCheck => {
                // Health check: just log, no response needed
                Ok(())
            }
        }
    }
}
```

**Compiler enforces**: Every `InFlight` variant must be handled in `complete()`. You cannot add a variant without handling its completion.

### State Transition Points

| Transition | Method | InFlight Variant | Completion Action |
|------------|--------|------------------|-------------------|
| Idle → Busy(Task) | `dispatch_to()` | `Task { respond_to }` | Send response to submitter |
| Idle → Busy(HealthCheck) | `dispatch_health_check()` | `HealthCheck` | Log success |
| Busy → Idle | `complete_task()` | Calls `in_flight.complete()` | Determined by variant |
| Busy(HealthCheck) → removed | timeout check | N/A | Deregister agent |

---

## Event Loop Mechanics

The daemon runs a polling event loop, not timers. Every ~100ms it:

```
loop {
    1. Check for socket submissions (non-blocking)
    2. Wait for FSEvents (100ms timeout)
    3. Handle any filesystem events (agent registration, response files)

    Every 500ms:
    4. scan_agents()           - detect new/removed agent directories
    5. scan_outputs()          - detect response.json files (completion)
    6. scan_pending()          - detect file-based submissions
    7. check_periodic_health_checks()  - NEW: dispatch to stale idle agents
    8. check_health_check_timeouts()   - NEW: remove timed-out agents

    9. dispatch_pending()      - assign queued tasks to idle agents
}
```

### How Completion Works

1. Agent writes `response.json` to its directory
2. FSEvents (or periodic scan) detects the file
3. `complete_task(agent_id, response_path)` is called:
   ```rust
   // Extract InFlight, set status to Idle
   let in_flight = std::mem::replace(&mut agent.status, AgentStatus::Idle);

   // Read response content
   let output = fs::read_to_string(response_path)?;

   // Clean up files
   fs::remove_file(task_path);
   fs::remove_file(response_path);

   // Typestate: InFlight determines what happens
   in_flight.complete(output)?;  // Task→send response, HealthCheck→log

   // Update activity timestamp
   agent.touch();
   ```

### How Timeout Works

No timers. Every 500ms, the event loop calls `check_health_check_timeouts()`:

```rust
fn check_health_check_timeouts(&mut self) {
    for (agent_id, agent) in &self.agents {
        // Only check agents doing health checks
        if !matches!(agent.status, AgentStatus::Busy(InFlight::HealthCheck)) {
            continue;
        }

        // Check if they've exceeded the timeout
        if agent.last_activity.elapsed() >= self.config.health_check_timeout {
            // Remove them entirely (not a state transition, just deletion)
            self.agents.remove(agent_id);
            fs::remove_dir_all(agent_dir);
        }
    }
}
```

**Key insight**: `last_activity` is set when we dispatch the health check. If 30 seconds pass without a response, `elapsed() >= 30s` becomes true on the next poll, and we remove the agent.

This is polling-based, not callback-based. The daemon doesn't "know" exactly when 30 seconds pass; it just checks on each loop iteration.

### With Tokio (Future State)

After the daemon refactor to tokio (see `DAEMON_REFACTOR.md`), timeouts would be event-driven:

```rust
// When dispatching health check, spawn a timeout future
let timeout_handle = tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(30)).await;
    timeout_tx.send(agent_id).await;
});

// In select!
tokio::select! {
    Some(agent_id) = timeout_rx.recv() => {
        // Check if still waiting (might have completed before timeout)
        if matches!(agent.status, AgentStatus::Busy(InFlight::HealthCheck)) {
            deregister_agent(agent_id);
        }
    }
}
```

**Difference**: Fires exactly at 30s, not "sometime within 500ms after 30s". More efficient (no polling), more precise.

---

## Current State

**AgentStatus enum** (already implemented):
```rust
enum AgentStatus {
    Idle,
    Busy(InFlight),
}

enum InFlight {
    Task { respond_to: ResponseTarget },
}

impl InFlight {
    fn complete(self, output: String) -> io::Result<()> {
        match self {
            InFlight::Task { respond_to } => {
                let response = Response::processed(output);
                send_response(respond_to, &response)
            }
        }
    }
}
```

**Task file envelope** (already implemented):
```json
{"kind": "Task", "content": {...original task...}}
```

The daemon writes this envelope; `get_task` passes through the `kind` field.

---

## Implementation Tasks

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 1 | Add `InFlight::HealthCheck` variant (compiler will force `complete()` update) |
| [ ] | 2 | Add health check config to `DaemonConfig` |
| [ ] | 3 | Add CLI flags for health check config |
| [x] | 4 | Task envelope format with `kind` field |
| [ ] | 5 | Add `dispatch_health_check()` (Idle → Busy(HealthCheck) transition) |
| [ ] | 6 | Update `register()` to send initial health check |
| [ ] | 7 | Add `last_activity` tracking to `AgentState` |
| [ ] | 8 | Add periodic health check dispatch (idle agents only) |
| [ ] | 9 | Add health check timeout handling (Busy(HealthCheck) → deregistered) |
| [ ] | 10 | Update shell scripts to handle HealthCheck kind |
| [ ] | 11 | Update demos |
| [ ] | 12 | Add tests |

---

### Task 1: Add `InFlight::HealthCheck` variant

**File:** `crates/agent_pool/src/daemon.rs`

**Change:**
```rust
enum InFlight {
    Task { respond_to: ResponseTarget },
    HealthCheck,  // NEW
}

impl InFlight {
    fn complete(self, output: String) -> io::Result<()> {
        match self {
            InFlight::Task { respond_to } => {
                let response = Response::processed(output);
                send_response(respond_to, &response)
            }
            InFlight::HealthCheck => {  // NEW - compiler forces this
                debug!(output, "health check completed");
                Ok(())
            }
        }
    }
}
```

**Typestate guarantee**: The compiler will not allow this code to compile until we add the `HealthCheck` arm to `complete()`. This ensures we cannot forget to handle health check completion.

---

### Task 2: Add health check config to `DaemonConfig`

**File:** `crates/agent_pool/src/daemon.rs`

```rust
pub struct DaemonConfig {
    pub initial_health_check: bool,      // Send health check on registration
    pub periodic_health_check: bool,     // Send periodic health checks to idle agents
    pub health_check_interval: Duration, // How often to check idle agents (default: 60s)
    pub health_check_timeout: Duration,  // How long to wait for response (default: 30s)
}
```

This config controls which state transitions are triggered automatically.

---

### Task 3: Add CLI flags for health check config

**File:** `crates/agent_pool/src/main.rs`

Add flags: `--initial-health-check`, `--periodic-health-check`, `--health-check-interval-secs`, `--health-check-timeout-secs`

Wire these to `DaemonConfig` and pass to `run_with_config()` / `spawn_with_config()`.

---

### Task 4: Task envelope format ✓

**Status: DONE**

The daemon writes `{"kind": "Task", "content": ...}` to task.json. The `get_task` CLI passes through the `kind` field. No changes needed for health checks - just write `{"kind": "HealthCheck", ...}`.

---

### Task 5: `dispatch_health_check()` — Idle → Busy(HealthCheck) transition

**File:** `crates/agent_pool/src/daemon.rs`

```rust
/// Transition: Idle → Busy(HealthCheck)
/// Writes health check task file, updates agent status.
fn dispatch_health_check(&mut self, agent_id: &str) -> io::Result<()> {
    let agent = self.agents.get_mut(agent_id)
        .ok_or_else(|| io::Error::other("agent not found"))?;

    // Write task file with HealthCheck kind
    let envelope = serde_json::json!({
        "kind": "HealthCheck",
        "content": { "instructions": "Respond with any value to confirm you are alive." }
    });
    let task_path = self.agents_dir.join(agent_id).join(TASK_FILE);
    fs::write(&task_path, envelope.to_string())?;

    // State transition: Idle → Busy(HealthCheck)
    agent.status = AgentStatus::Busy(InFlight::HealthCheck);
    agent.touch(); // Reset activity timer
    Ok(())
}
```

**Precondition**: Agent must be Idle (caller's responsibility to check).

---

### Task 6: Initial health check on registration

**File:** `crates/agent_pool/src/daemon.rs`

When agent registers, optionally trigger Idle → Busy(HealthCheck):

```rust
fn register(&mut self, agent_id: &str) {
    if self.agents.contains_key(agent_id) {
        return;
    }

    self.agents.insert(agent_id.to_string(), AgentState::new());

    if self.config.initial_health_check {
        let _ = self.dispatch_health_check(agent_id); // Idle → Busy(HealthCheck)
    }
}
```

---

### Task 7: Add `last_activity` tracking to `AgentState`

**File:** `crates/agent_pool/src/daemon.rs`

```rust
struct AgentState {
    status: AgentStatus,
    last_activity: Instant,  // NEW: for periodic check + timeout
}

impl AgentState {
    fn new() -> Self {
        Self { status: AgentStatus::Idle, last_activity: Instant::now() }
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }
}
```

**Touch points**:
- `AgentState::new()` - set to now
- `dispatch_health_check()` - touch when dispatching
- `complete_task()` - touch when completing (Busy → Idle)

---

### Task 8: Periodic health check dispatch

**File:** `crates/agent_pool/src/daemon.rs`

```rust
/// Check idle agents and dispatch health checks if stale.
/// Transition: Idle → Busy(HealthCheck) for stale agents
fn check_periodic_health_checks(&mut self) -> io::Result<()> {
    if !self.config.periodic_health_check {
        return Ok(());
    }

    let interval = self.config.health_check_interval;
    let stale: Vec<_> = self.agents.iter()
        .filter(|(_, a)| a.is_idle() && a.last_activity.elapsed() >= interval)
        .map(|(id, _)| id.clone())
        .collect();

    for agent_id in stale {
        let _ = self.dispatch_health_check(&agent_id);
    }
    Ok(())
}
```

**Called from**: Event loop, after `scan_pending()`.

---

### Task 9: Health check timeout — Busy(HealthCheck) → deregistered

**File:** `crates/agent_pool/src/daemon.rs`

```rust
/// Check for health check timeouts.
/// Transition: Busy(HealthCheck) → removed (if timed out)
fn check_health_check_timeouts(&mut self) {
    let timeout = self.config.health_check_timeout;

    let timed_out: Vec<_> = self.agents.iter()
        .filter(|(_, a)| {
            matches!(a.status, AgentStatus::Busy(InFlight::HealthCheck))
                && a.last_activity.elapsed() >= timeout
        })
        .map(|(id, _)| id.clone())
        .collect();

    for agent_id in timed_out {
        warn!(agent_id, "health check timeout, deregistering");
        let _ = fs::remove_dir_all(self.agents_dir.join(&agent_id));
        self.agents.remove(&agent_id);
    }
}
```

**Note**: This is NOT a normal state transition. Timeout removes the agent entirely. Agent can recover by calling `get_task` again (re-registers).

**Called from**: Event loop, after `check_periodic_health_checks()`.

---

### Task 10: Update shell scripts

**Files:** `crates/agent_pool/scripts/*.sh`

Add at start of task loop:
```bash
KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')
if [ "$KIND" = "HealthCheck" ]; then
    echo "{}" > "$RESPONSE_FILE"
    continue
fi
```

---

### Task 11: Update demos

**Files:** `crates/agent_pool/demos/*.sh`

Disable health checks for simplicity:
```bash
agent_pool start --pool "$POOL" \
    --initial-health-check=false \
    --periodic-health-check=false &
```

---

### Task 12: Add tests

**File:** `crates/agent_pool/tests/health_check.rs`

Test the state machine:
1. Registration triggers Idle → Busy(HealthCheck) when enabled
2. Health check response triggers Busy(HealthCheck) → Idle
3. Periodic check only affects Idle agents (not Busy(Task))
4. Timeout removes agent entirely
5. Removed agent can re-register
6. Disabled config skips health checks

---

## Summary

### State Transitions Added

| Transition | Trigger | Completion |
|------------|---------|------------|
| Idle → Busy(HealthCheck) | `dispatch_health_check()` | - |
| Busy(HealthCheck) → Idle | `complete_task()` | `InFlight::complete()` logs |
| Busy(HealthCheck) → removed | timeout | Deregister agent |

### Typestate Guarantees

1. **Exhaustive match in `complete()`**: Adding `InFlight::HealthCheck` won't compile until handled
2. **Single completion point**: All Busy → Idle transitions go through `complete_task()` → `InFlight::complete()`
3. **Variant determines action**: `Task` sends response, `HealthCheck` just logs

### Files Changed

| File | Changes |
|------|---------|
| `daemon.rs` | `InFlight::HealthCheck`, `DaemonConfig`, `AgentState.last_activity`, dispatch/timeout methods |
| `main.rs` | CLI flags for health check config |
| `scripts/*.sh` | Handle HealthCheck kind |
| `demos/*.sh` | Disable health checks |

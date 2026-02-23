//! Layer 1: Pure State Machine
//!
//! This module contains the core state machine logic with zero I/O dependencies.
//! All types are pure data, and the `step` function is a pure transformation:
//!
//! ```text
//! step(state, event) -> (state, effects)
//! ```
//!
//! Key design principles:
//! - **No I/O**: No filesystem, no sockets, no time
//! - **No config**: Layer 1 is purely reactive
//! - **IDs only**: Layer 3 maps IDs to actual content/channels
//! - **Deterministic**: Same input always produces same output
//!
//! # Epoch-Based Timeout Validation
//!
//! Timeouts are tricky in distributed systems. A timer might fire after the
//! agent has already done work, making the timeout stale. We handle this with
//! epochs:
//!
//! - Each agent has an `Epoch` (`agent_id` + sequence number)
//! - Sequence increments on every state transition (idle→busy, busy→idle)
//! - Timeout events carry the epoch they were created with
//! - If current epoch doesn't match, the timeout is stale and ignored

use std::collections::{BTreeMap, VecDeque};

// =============================================================================
// ID Types
// =============================================================================

/// Unique identifier for a task submission.
///
/// Tasks are tracked by ID throughout their lifecycle:
/// submitted → dispatched → completed/failed
///
/// Layer 3 maps `TaskId` to actual content and response channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub u32);

/// Unique identifier for a registered agent.
///
/// Agents are anonymous - they have no "name", just an ID assigned on registration.
/// Layer 3 maps `AgentId` to the actual communication channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(pub u32);

/// Agent epoch - identifies a specific point in an agent's lifecycle.
///
/// Used to validate timeout events. Contains:
/// - `agent_id`: Which agent this epoch belongs to
/// - `sequence`: Increments on every state transition
///
/// When a timeout fires, we check if the agent's current epoch matches.
/// If not, the agent has done work since the timer started, so we ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Epoch {
    /// Which agent this epoch belongs to.
    pub agent_id: AgentId,
    /// Increments on every state transition (idle→busy, busy→idle).
    pub sequence: u32,
}

// =============================================================================
// Agent State
// =============================================================================

/// What an agent is currently doing.
///
/// Note: Layer 1 doesn't distinguish between "real tasks" and "health checks".
/// That's a Layer 3 concern. From Layer 1's perspective, an agent is either
/// idle (ready for work) or busy (processing something).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Ready to receive work.
    Idle,
    /// Currently processing a task.
    Busy {
        /// The task being processed.
        task_id: TaskId,
    },
}

/// Complete state for a single agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentState {
    /// What the agent is currently doing
    pub status: AgentStatus,
    /// Current epoch - increments on every state transition
    pub epoch: Epoch,
}

impl AgentState {
    /// Create a new agent in idle state with initial epoch.
    const fn new(agent_id: AgentId) -> Self {
        Self {
            status: AgentStatus::Idle,
            epoch: Epoch {
                agent_id,
                sequence: 0,
            },
        }
    }

    /// Check if the agent is idle.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }

    /// Transition from idle to busy. Returns the new epoch.
    ///
    /// # Panics
    /// Panics if agent is not idle.
    fn become_busy(&mut self, task_id: TaskId) -> Epoch {
        debug_assert!(self.is_idle(), "become_busy called on non-idle agent");
        self.epoch.sequence += 1;
        self.status = AgentStatus::Busy { task_id };
        self.epoch
    }

    /// Transition from busy to idle. Returns the new epoch.
    ///
    /// # Panics
    /// Panics if agent is not busy.
    fn become_idle(&mut self) -> Epoch {
        debug_assert!(!self.is_idle(), "become_idle called on idle agent");
        self.epoch.sequence += 1;
        self.status = AgentStatus::Idle;
        self.epoch
    }
}

// =============================================================================
// Pool State
// =============================================================================

/// The complete state of the agent pool.
///
/// This is the "world" that the state machine operates on. It contains:
/// - A queue of pending tasks (just IDs - content is in Layer 3)
/// - A map of registered agents and their states
///
/// `BTreeMap` is used for agents to ensure deterministic iteration order,
/// which helps with debugging and snapshot testing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolState {
    /// Tasks waiting to be assigned to agents (FIFO queue)
    pending_tasks: VecDeque<TaskId>,
    /// Registered agents and their current state
    agents: BTreeMap<AgentId, AgentState>,
}

impl PoolState {
    /// Create a new empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of pending tasks.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending_tasks.len()
    }

    /// Number of registered agents.
    #[must_use]
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Number of busy agents.
    #[must_use]
    pub fn busy_count(&self) -> usize {
        self.agents.values().filter(|a| !a.is_idle()).count()
    }

    /// Number of idle agents.
    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.agents.values().filter(|a| a.is_idle()).count()
    }

    /// Check if an agent is registered.
    #[must_use]
    pub fn has_agent(&self, agent_id: AgentId) -> bool {
        self.agents.contains_key(&agent_id)
    }

    /// Get agent state (for testing).
    #[must_use]
    pub fn get_agent(&self, agent_id: AgentId) -> Option<&AgentState> {
        self.agents.get(&agent_id)
    }

    /// Check if a task is pending (for testing).
    #[must_use]
    pub fn has_pending_task(&self, task_id: TaskId) -> bool {
        self.pending_tasks.contains(&task_id)
    }
}

// =============================================================================
// Events (Inputs)
// =============================================================================

/// Events that can affect pool state.
///
/// Named in past tense - these are things that HAPPENED.
/// Layer 3 detects these events and sends them to Layer 1.
///
/// Note: Events carry minimal data. Content, channels, and other
/// "real world" concerns live in Layer 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A task was submitted.
    /// Layer 3 has stored the content and response channel.
    TaskSubmitted {
        /// The submitted task's ID.
        task_id: TaskId,
    },

    /// A task was withdrawn (submitter disconnected before completion).
    /// Remove from pending queue if not yet dispatched.
    TaskWithdrawn {
        /// The withdrawn task's ID.
        task_id: TaskId,
    },

    /// An agent registered (directory appeared or socket connected).
    /// Layer 3 assigns the `AgentId`.
    AgentRegistered {
        /// The registered agent's ID.
        agent_id: AgentId,
    },

    /// An agent deregistered (directory removed or socket closed).
    AgentDeregistered {
        /// The deregistered agent's ID.
        agent_id: AgentId,
    },

    /// An agent completed its work (wrote `response.json` or sent response).
    /// Layer 3 has stored the response content.
    AgentResponded {
        /// The responding agent's ID.
        agent_id: AgentId,
    },

    /// A timeout fired for an agent.
    /// Layer 1 checks if the epoch matches; if not, ignores the stale timeout.
    AgentTimedOut {
        /// The epoch when the timer was started.
        epoch: Epoch,
    },
}

// =============================================================================
// Effects (Outputs)
// =============================================================================

/// Actions for Layer 3 to execute.
///
/// Effects are the "commands" that Layer 1 emits. Layer 3 interprets them
/// and performs the actual I/O.
///
/// Note: Effects carry minimal data. Layer 3 looks up content, channels,
/// and other details using the IDs provided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Dispatch a task to an agent.
    /// Layer 3: write `task.json`, start timeout timer using epoch.
    DispatchTask {
        /// The task to dispatch.
        task_id: TaskId,
        /// The agent's epoch when dispatched (for timeout validation).
        epoch: Epoch,
    },

    /// Agent became idle (after registration or task completion).
    /// Layer 3: start idle timeout timer using epoch.
    AgentBecameIdle {
        /// The agent's current epoch (for timeout validation).
        epoch: Epoch,
    },

    /// Task completed successfully.
    /// Layer 3: read response from agent, send to submitter.
    TaskCompleted {
        /// The agent that completed the task.
        agent_id: AgentId,
        /// The completed task.
        task_id: TaskId,
    },

    /// Task failed (agent timed out while processing).
    /// Layer 3: send error response to submitter.
    TaskFailed {
        /// The failed task.
        task_id: TaskId,
    },

    /// Deregister an agent (remove directory, close socket).
    /// Layer 3: clean up the agent's channel.
    DeregisterAgent {
        /// The agent to deregister.
        agent_id: AgentId,
    },
}

// =============================================================================
// Step Function
// =============================================================================

/// Pure state transition function.
///
/// This is the heart of Layer 1. Given the current state and an event,
/// it returns the new state and a list of effects to execute.
///
/// Properties:
/// - **Pure**: No I/O, no side effects
/// - **Deterministic**: Same inputs always produce same outputs
/// - **Total**: Handles any event in any state (Byzantine resilient)
#[must_use]
pub fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::TaskSubmitted { task_id } => handle_task_submitted(state, task_id),
        Event::TaskWithdrawn { task_id } => handle_task_withdrawn(state, task_id),
        Event::AgentRegistered { agent_id } => handle_agent_registered(state, agent_id),
        Event::AgentDeregistered { agent_id } => handle_agent_deregistered(state, agent_id),
        Event::AgentResponded { agent_id } => handle_agent_responded(state, agent_id),
        Event::AgentTimedOut { epoch } => handle_agent_timed_out(state, epoch),
    }
}

// =============================================================================
// Event Handlers
// =============================================================================

fn handle_task_submitted(mut state: PoolState, task_id: TaskId) -> (PoolState, Vec<Effect>) {
    state.pending_tasks.push_back(task_id);
    let effects = try_dispatch(&mut state);
    (state, effects)
}

fn handle_task_withdrawn(mut state: PoolState, task_id: TaskId) -> (PoolState, Vec<Effect>) {
    // Remove from pending queue if not yet dispatched
    state.pending_tasks.retain(|&id| id != task_id);
    // If already dispatched, we can't recall it. The response will be
    // discarded when TaskCompleted is processed (Layer 3 won't find a responder).
    (state, vec![])
}

fn handle_agent_registered(mut state: PoolState, agent_id: AgentId) -> (PoolState, Vec<Effect>) {
    // Idempotent: ignore duplicate registration
    if state.agents.contains_key(&agent_id) {
        return (state, vec![]);
    }

    let agent = AgentState::new(agent_id);
    let epoch = agent.epoch;
    state.agents.insert(agent_id, agent);

    let mut effects = vec![Effect::AgentBecameIdle { epoch }];
    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_agent_deregistered(mut state: PoolState, agent_id: AgentId) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.remove(&agent_id) else {
        // Agent not registered, nothing to do
        return (state, vec![]);
    };

    // If agent was busy, fail the task
    let mut effects = vec![];
    if let AgentStatus::Busy { task_id } = agent.status {
        effects.push(Effect::TaskFailed { task_id });
    }

    (state, effects)
}

fn handle_agent_responded(mut state: PoolState, agent_id: AgentId) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.get_mut(&agent_id) else {
        // Unknown agent responded - ignore (Byzantine resilience)
        return (state, vec![]);
    };

    let AgentStatus::Busy { task_id } = agent.status else {
        // Agent not busy - ignore (Byzantine resilience)
        return (state, vec![]);
    };

    let new_epoch = agent.become_idle();

    let mut effects = vec![
        Effect::TaskCompleted { agent_id, task_id },
        Effect::AgentBecameIdle { epoch: new_epoch },
    ];

    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_agent_timed_out(mut state: PoolState, epoch: Epoch) -> (PoolState, Vec<Effect>) {
    let agent_id = epoch.agent_id;

    let Some(agent) = state.agents.get(&agent_id) else {
        // Agent already gone, ignore stale timeout
        return (state, vec![]);
    };

    if agent.epoch != epoch {
        // Epoch mismatch - agent did work since timer started, ignore
        return (state, vec![]);
    }

    // Capture task_id if agent was busy
    let in_flight_task = match agent.status {
        AgentStatus::Busy { task_id } => Some(task_id),
        AgentStatus::Idle => None,
    };

    // Remove the agent
    state.agents.remove(&agent_id);

    let mut effects = vec![Effect::DeregisterAgent { agent_id }];

    // If agent was busy, fail the task (insert at front for logical ordering)
    if let Some(task_id) = in_flight_task {
        effects.insert(0, Effect::TaskFailed { task_id });
    }

    (state, effects)
}

// =============================================================================
// Dispatch Logic
// =============================================================================

/// Try to dispatch pending tasks to idle agents.
///
/// Returns effects for each successful dispatch.
#[allow(clippy::expect_used)] // Internal invariant: find_idle_agent only returns existing agents
fn try_dispatch(state: &mut PoolState) -> Vec<Effect> {
    let mut effects = vec![];

    while let Some(agent_id) = find_idle_agent(state) {
        let Some(task_id) = state.pending_tasks.pop_front() else {
            break;
        };

        let agent = state
            .agents
            .get_mut(&agent_id)
            .expect("find_idle_agent returned unknown agent");

        let epoch = agent.become_busy(task_id);
        effects.push(Effect::DispatchTask { task_id, epoch });
    }

    effects
}

/// Find an idle agent, if any.
///
/// Returns the first idle agent found. `BTreeMap` ensures deterministic order.
fn find_idle_agent(state: &PoolState) -> Option<AgentId> {
    state
        .agents
        .iter()
        .find(|(_, agent)| agent.is_idle())
        .map(|(&id, _)| id)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Task Submission Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_submitted_queues_when_no_agents() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(1) });

        assert_eq!(state.pending_count(), 1);
        assert!(state.has_pending_task(TaskId(1)));
        assert!(effects.is_empty(), "No agents, so no dispatch");
    }

    #[test]
    fn task_submitted_dispatches_to_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(42) });

        assert_eq!(state.pending_count(), 0, "Task should be dispatched");
        assert_eq!(state.busy_count(), 1);

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert_eq!(agent.status, AgentStatus::Busy { task_id: TaskId(42) });
        assert_eq!(agent.epoch.sequence, 1, "Epoch should increment on dispatch");

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::DispatchTask { task_id: TaskId(42), epoch }
                if epoch.agent_id == AgentId(1) && epoch.sequence == 1
        ));
    }

    #[test]
    fn task_submitted_queues_when_all_agents_busy() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.become_busy(TaskId(99));
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(42) });

        assert_eq!(state.pending_count(), 1);
        assert!(state.has_pending_task(TaskId(42)));
        assert!(effects.is_empty());
    }

    #[test]
    fn multiple_tasks_dispatch_to_multiple_agents() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        state.agents.insert(AgentId(2), AgentState::new(AgentId(2)));
        state.pending_tasks.push_back(TaskId(1));
        state.pending_tasks.push_back(TaskId(2));
        state.pending_tasks.push_back(TaskId(3));

        // Trigger dispatch by submitting another task
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(4) });

        // 2 agents, 4 tasks -> 2 dispatched, 2 pending
        assert_eq!(state.busy_count(), 2);
        assert_eq!(state.pending_count(), 2);
        assert_eq!(effects.len(), 2);
    }

    // -------------------------------------------------------------------------
    // Task Withdrawal Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_withdrawn_removes_from_pending() {
        let mut state = PoolState::new();
        state.pending_tasks.push_back(TaskId(1));
        state.pending_tasks.push_back(TaskId(2));
        state.pending_tasks.push_back(TaskId(3));

        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: TaskId(2) });

        assert_eq!(state.pending_count(), 2);
        assert!(!state.has_pending_task(TaskId(2)));
        assert!(state.has_pending_task(TaskId(1)));
        assert!(state.has_pending_task(TaskId(3)));
        assert!(effects.is_empty());
    }

    #[test]
    fn task_withdrawn_noop_for_unknown_task() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: TaskId(999) });

        assert_eq!(state.pending_count(), 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn task_withdrawn_noop_for_dispatched_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.become_busy(TaskId(42));
        state.agents.insert(AgentId(1), agent);

        // Try to withdraw a task that's already dispatched
        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: TaskId(42) });

        // Task is still being processed - can't recall it
        assert_eq!(state.busy_count(), 1);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Agent Registration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_registered_adds_to_pool() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::AgentRegistered { agent_id: AgentId(1) });

        assert!(state.has_agent(AgentId(1)));
        assert_eq!(state.idle_count(), 1);

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::AgentBecameIdle { epoch }
                if epoch.agent_id == AgentId(1) && epoch.sequence == 0
        ));
    }

    #[test]
    fn agent_registered_dispatches_pending_task() {
        let mut state = PoolState::new();
        state.pending_tasks.push_back(TaskId(42));

        let (state, effects) = step(state, Event::AgentRegistered { agent_id: AgentId(1) });

        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.busy_count(), 1);

        // Should have AgentBecameIdle (initial) + DispatchTask
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[0], Effect::AgentBecameIdle { .. }));
        assert!(matches!(&effects[1], Effect::DispatchTask { task_id: TaskId(42), .. }));
    }

    #[test]
    fn agent_registered_idempotent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(state, Event::AgentRegistered { agent_id: AgentId(1) });

        assert_eq!(state.agent_count(), 1);
        assert!(effects.is_empty(), "Duplicate registration should be no-op");
    }

    // -------------------------------------------------------------------------
    // Agent Deregistration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_deregistered_removes_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(state, Event::AgentDeregistered { agent_id: AgentId(1) });

        assert!(!state.has_agent(AgentId(1)));
        assert!(effects.is_empty(), "Idle agent deregister has no effects");
    }

    #[test]
    fn agent_deregistered_fails_in_flight_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.become_busy(TaskId(42));
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::AgentDeregistered { agent_id: AgentId(1) });

        assert!(!state.has_agent(AgentId(1)));
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::TaskFailed { task_id: TaskId(42) }));
    }

    #[test]
    fn agent_deregistered_noop_for_unknown() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::AgentDeregistered { agent_id: AgentId(999) });

        assert_eq!(state.agent_count(), 0);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Agent Response Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_responded_completes_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.become_busy(TaskId(42));
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::AgentResponded { agent_id: AgentId(1) });

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert!(agent.is_idle());
        assert_eq!(agent.epoch.sequence, 2, "Epoch should be 2 (busy + idle)");

        assert_eq!(effects.len(), 2);
        assert!(matches!(
            &effects[0],
            Effect::TaskCompleted { agent_id: AgentId(1), task_id: TaskId(42) }
        ));
        assert!(matches!(
            &effects[1],
            Effect::AgentBecameIdle { epoch } if epoch.sequence == 2
        ));
    }

    #[test]
    fn agent_responded_dispatches_next_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.become_busy(TaskId(1));
        state.agents.insert(AgentId(1), agent);
        state.pending_tasks.push_back(TaskId(2));

        let (state, effects) = step(state, Event::AgentResponded { agent_id: AgentId(1) });

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert_eq!(agent.status, AgentStatus::Busy { task_id: TaskId(2) });
        assert_eq!(agent.epoch.sequence, 3, "Epoch: 1 (busy) + 2 (idle) + 3 (busy)");

        assert_eq!(effects.len(), 3);
        assert!(matches!(&effects[0], Effect::TaskCompleted { .. }));
        assert!(matches!(&effects[1], Effect::AgentBecameIdle { .. }));
        assert!(matches!(&effects[2], Effect::DispatchTask { task_id: TaskId(2), .. }));
    }

    #[test]
    fn agent_responded_noop_for_unknown_agent() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::AgentResponded { agent_id: AgentId(999) });

        assert!(effects.is_empty());
        assert_eq!(state.agent_count(), 0);
    }

    #[test]
    fn agent_responded_noop_for_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(state, Event::AgentResponded { agent_id: AgentId(1) });

        assert!(state.get_agent(AgentId(1)).unwrap().is_idle());
        assert_eq!(state.get_agent(AgentId(1)).unwrap().epoch.sequence, 0);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Timeout Tests
    // -------------------------------------------------------------------------

    #[test]
    fn timeout_deregisters_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        let epoch = state.get_agent(AgentId(1)).unwrap().epoch;

        let (state, effects) = step(state, Event::AgentTimedOut { epoch });

        assert!(!state.has_agent(AgentId(1)));
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::DeregisterAgent { agent_id: AgentId(1) }));
    }

    #[test]
    fn timeout_deregisters_busy_agent_and_fails_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        let epoch = agent.become_busy(TaskId(42));
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::AgentTimedOut { epoch });

        assert!(!state.has_agent(AgentId(1)));
        assert_eq!(effects.len(), 2);
        // TaskFailed should come first (logical ordering)
        assert!(matches!(&effects[0], Effect::TaskFailed { task_id: TaskId(42) }));
        assert!(matches!(&effects[1], Effect::DeregisterAgent { agent_id: AgentId(1) }));
    }

    #[test]
    fn stale_timeout_ignored_epoch_mismatch() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        let old_epoch = agent.epoch;
        agent.become_busy(TaskId(42)); // Epoch is now 1
        state.agents.insert(AgentId(1), agent);

        // Fire timeout with old epoch (sequence 0)
        let (state, effects) = step(state, Event::AgentTimedOut { epoch: old_epoch });

        // Agent should still exist
        assert!(state.has_agent(AgentId(1)));
        assert!(effects.is_empty());
    }

    #[test]
    fn stale_timeout_ignored_agent_gone() {
        let state = PoolState::new();
        let epoch = Epoch {
            agent_id: AgentId(999),
            sequence: 0,
        };

        let (state, effects) = step(state, Event::AgentTimedOut { epoch });

        assert!(effects.is_empty());
        assert_eq!(state.agent_count(), 0);
    }

    // -------------------------------------------------------------------------
    // Epoch Sequence Tests
    // -------------------------------------------------------------------------

    #[test]
    fn epoch_increments_correctly_through_lifecycle() {
        let state = PoolState::new();

        // Register agent - epoch 0
        let (mut state, effects) = step(state, Event::AgentRegistered { agent_id: AgentId(1) });
        let epoch0 = match &effects[0] {
            Effect::AgentBecameIdle { epoch } => *epoch,
            _ => panic!("Expected AgentBecameIdle"),
        };
        assert_eq!(epoch0.sequence, 0);

        // Submit task - agent becomes busy, epoch 1
        state.pending_tasks.push_back(TaskId(1));
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(2) });
        // First dispatch is task 1
        let epoch1 = match &effects[0] {
            Effect::DispatchTask { epoch, .. } => *epoch,
            _ => panic!("Expected DispatchTask"),
        };
        assert_eq!(epoch1.sequence, 1);

        // Agent responds - becomes idle, epoch 2
        let (_state, effects) = step(state, Event::AgentResponded { agent_id: AgentId(1) });
        let epoch2 = match &effects[1] {
            Effect::AgentBecameIdle { epoch } => *epoch,
            _ => panic!("Expected AgentBecameIdle"),
        };
        assert_eq!(epoch2.sequence, 2);

        // Dispatched task 2 - becomes busy, epoch 3
        let epoch3 = match &effects[2] {
            Effect::DispatchTask { epoch, .. } => *epoch,
            _ => panic!("Expected DispatchTask"),
        };
        assert_eq!(epoch3.sequence, 3);
    }

    // -------------------------------------------------------------------------
    // Determinism Tests
    // -------------------------------------------------------------------------

    #[test]
    fn dispatch_order_is_deterministic() {
        // Create state with multiple agents and tasks
        let mut state = PoolState::new();
        state.agents.insert(AgentId(3), AgentState::new(AgentId(3)));
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        state.agents.insert(AgentId(2), AgentState::new(AgentId(2)));
        state.pending_tasks.push_back(TaskId(10));
        state.pending_tasks.push_back(TaskId(20));

        let (_state, effects) = step(state, Event::TaskSubmitted { task_id: TaskId(30) });

        // BTreeMap ensures agents are iterated in ID order: 1, 2, 3
        // VecDeque ensures tasks are dispatched FIFO: 10, 20, 30
        let dispatches: Vec<_> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::DispatchTask { task_id, epoch } => Some((*task_id, epoch.agent_id)),
                _ => None,
            })
            .collect();

        assert_eq!(
            dispatches,
            vec![
                (TaskId(10), AgentId(1)),
                (TaskId(20), AgentId(2)),
                (TaskId(30), AgentId(3)),
            ]
        );
    }
}

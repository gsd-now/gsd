//! V2 Agent Pool Implementation
//!
//! A three-layer architecture for the agent pool daemon:
//!
//! - **Layer 1 (core)**: Pure state machine - no I/O, no time, fully testable
//! - **Layer 2 (`event_loop`)**: Orchestration - receives events, calls step, sends effects
//! - **Layer 3 (io)**: I/O - sockets, filesystem, timers
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Layer 3: I/O                                 │
//! │  - Socket accept, filesystem events, timer management           │
//! │  - Maps IDs to actual channels and content                      │
//! └────────────────────────────────┬────────────────────────────────┘
//!                                  │ Events
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Layer 2: Event Loop                          │
//! │  - Holds PoolState                                              │
//! │  - Calls step(state, event) → (state, effects)                  │
//! └────────────────────────────────┬────────────────────────────────┘
//!                                  │ Effects
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Layer 1: Pure State Machine                  │
//! │  - fn step(state, event) → (state, effects)                     │
//! │  - No I/O, no time, completely deterministic                    │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Event and Effect Flow
//!
//! Events flow inward: Layer 3 detects something (FS event, socket, timer),
//! creates an Event, sends to Layer 2, which passes to Layer 1.
//!
//! Effects flow outward: Layer 1 returns Effects, Layer 2 sends them to
//! Layer 3, which executes the actual I/O.
//!
//! # TODO
//!
//! - [ ] Event/effect recording for replay and snapshot testing

pub mod core;

pub use self::core::{
    AgentId, AgentState, AgentStatus, Effect, Epoch, Event, PoolState, TaskId, step,
};

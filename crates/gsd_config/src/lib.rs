//! GSD Config - Declarative Task Orchestration
//!
//! Config-based task orchestrator that sits on top of `agent_pool`.
//!
//! # Overview
//!
//! Define task workflows via a declarative config. This crate:
//! - Validates tasks against JSON schemas
//! - Generates markdown documentation for agents
//! - Handles timeouts and retries with per-step options
//!
//! The config format is serialization-agnostic (uses serde). The CLI
//! handles parsing from JSON or other formats.
//!
//! # Task Format
//!
//! Tasks have a `kind` (step name) and `value` (payload).
//! Agents return arrays of tasks as their response.

pub mod config;
mod docs;
pub mod maybe_linked;
pub mod resolved;
mod runner;
mod types;
mod value_schema;

// Public API - only what external users need
// Config file types (for parsing)
pub use config::{ConfigFile, config_schema};
// Resolved types (for runtime)
pub use docs::generate_full_docs;
pub use maybe_linked::MaybeLinked;
pub use resolved::{Action, Config, Options, Step};
pub use runner::{RunnerConfig, TaskOutcome, TaskResult, TaskRunner, run};
pub use types::StepName;
pub use value_schema::{CompiledSchemas, Task};

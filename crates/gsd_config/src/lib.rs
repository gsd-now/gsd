//! GSD Config - Declarative Task Orchestration
//!
//! Config-based task orchestrator that sits on top of `agent_pool`.
//!
//! # Overview
//!
//! Define task state machines via a declarative config. This crate:
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
pub mod docs;
pub mod runner;
pub mod schema;

pub use config::{Config, ConfigError, EffectiveOptions, Options, SchemaRef, Step, StepOptions};
pub use docs::{generate_full_docs, generate_step_docs};
pub use runner::{RunnerConfig, run};
pub use schema::{CompiledSchemas, ResponseValidationError, Task, ValidationError};

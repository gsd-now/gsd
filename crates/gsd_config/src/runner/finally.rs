//! Finally hook tracking and execution.

use std::collections::HashMap;
use tracing::{info, warn};

use crate::types::{HookScript, LogTaskId};
use crate::value_schema::Task;

use super::shell::run_shell_command;

pub struct FinallyState {
    pub pending_count: usize,
    pub original_value: serde_json::Value,
    pub finally_command: HookScript,
}

pub struct FinallyTracker {
    tracking: HashMap<LogTaskId, FinallyState>,
}

impl FinallyTracker {
    pub fn new() -> Self {
        Self {
            tracking: HashMap::new(),
        }
    }

    pub fn start_tracking(
        &mut self,
        task_id: LogTaskId,
        pending_count: usize,
        original_value: serde_json::Value,
        finally_command: HookScript,
    ) {
        self.tracking.insert(
            task_id,
            FinallyState {
                pending_count,
                original_value,
                finally_command,
            },
        );
    }

    /// Record that a descendant of `origin_id` has completed.
    ///
    /// Returns `Some(FinallyState)` when all descendants are done and the
    /// finally hook is ready to run. Returns `None` if descendants remain
    /// or if `origin_id` has no finally tracking (no-op for tasks without finally hooks).
    pub fn record_descendant_done(&mut self, origin_id: LogTaskId) -> Option<FinallyState> {
        let ready_for_finally = if let Some(state) = self.tracking.get_mut(&origin_id) {
            state.pending_count = state.pending_count.saturating_sub(1);
            state.pending_count == 0
        } else {
            // Not tracked - origin has no finally hook, this is expected
            return None;
        };

        if ready_for_finally {
            self.tracking.remove(&origin_id)
        } else {
            None
        }
    }
}

pub fn run_finally_hook(state: &FinallyState) -> Vec<Task> {
    info!(command = %state.finally_command, "running finally hook");

    let input_json = serde_json::to_string(&state.original_value).unwrap_or_default();

    match run_shell_command(state.finally_command.as_str(), &input_json, None) {
        Ok(stdout) => match serde_json::from_str::<Vec<Task>>(&stdout) {
            Ok(tasks) => {
                info!(count = tasks.len(), "finally hook spawned tasks");
                tasks
            }
            Err(e) => {
                warn!(error = %e, "finally hook output is not valid JSON (ignored)");
                vec![]
            }
        },
        Err(e) => {
            warn!(error = %e, "finally hook failed (ignored)");
            vec![]
        }
    }
}

//! Task dispatch - spawns threads to execute pool and command tasks.

use std::path::Path;
use std::sync::mpsc;

use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use tracing::debug;

use crate::types::{HookScript, StepInputValue};

use super::hooks::{run_command_action, run_pre_hook};
use super::submit::{build_agent_payload, submit_via_cli};
use super::types::{InFlightResult, SubmitResult, TaskIdentity};

/// Context for dispatching a task.
pub struct TaskContext {
    pub identity: TaskIdentity,
    pub pre_hook: Option<HookScript>,
}

/// Run pre-hook if present, returning the value or sending an error result.
///
/// Returns `Some(StepInputValue)` to continue processing, `None` if error was sent.
fn run_pre_hook_or_send_error(
    ctx: &TaskContext,
    original_value: &StepInputValue,
    tx: &mpsc::Sender<InFlightResult>,
) -> Option<StepInputValue> {
    let Some(hook) = &ctx.pre_hook else {
        // No pre-hook, original value passes through unchanged
        return Some(original_value.clone());
    };

    match run_pre_hook(hook, &original_value.0) {
        Ok(v) => Some(StepInputValue(v)),
        Err(e) => {
            let _ = tx.send(InFlightResult {
                identity: ctx.identity.clone(),
                result: SubmitResult::PreHookError(e),
            });
            None
        }
    }
}

/// Execute a pool task (runs in spawned thread).
pub fn dispatch_pool_task(
    ctx: TaskContext,
    docs: &str,
    timeout: Option<u64>,
    pool_root: &Path,
    invoker: &Invoker<AgentPoolCli>,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let original_value = &ctx.identity.task.value;

    let Some(value) = run_pre_hook_or_send_error(&ctx, original_value, tx) else {
        return;
    };

    let payload = build_agent_payload(&ctx.identity.task.step, &value.0, docs, timeout);
    debug!(payload = %payload, "task payload");

    let response = submit_via_cli(pool_root, &payload, invoker);
    let _ = tx.send(InFlightResult {
        identity: ctx.identity,
        result: SubmitResult::Pool { value, response },
    });
}

/// Execute a command task (runs in spawned thread).
pub fn dispatch_command_task(
    ctx: TaskContext,
    script: &str,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let original_value = &ctx.identity.task.value;

    let Some(value) = run_pre_hook_or_send_error(&ctx, original_value, tx) else {
        return;
    };

    let task_json = serde_json::to_string(&serde_json::json!({
        "kind": &ctx.identity.task.step,
        "value": &value.0,
    }))
    .unwrap_or_default();

    let output = run_command_action(script, &task_json, working_dir);
    let _ = tx.send(InFlightResult {
        identity: ctx.identity,
        result: SubmitResult::Command { value, output },
    });
}

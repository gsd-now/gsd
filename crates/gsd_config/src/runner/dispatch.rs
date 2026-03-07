//! Task dispatch - spawns threads to execute pool and command tasks.

use std::io;
use std::path::Path;
use std::sync::mpsc;

use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use tracing::debug;

use crate::types::{LogTaskId, StepName};
use crate::value_schema::Task;

use super::hooks::{run_command_action, run_pre_hook};
use super::submit::{build_agent_payload, submit_via_cli};
use super::types::{InFlightResult, SubmitResult};

/// Common context for dispatching a task.
pub struct TaskContext {
    pub task: Task,
    pub task_id: LogTaskId,
    pub origin_id: Option<LogTaskId>,
    pub step_name: StepName,
    pub pre_hook: Option<String>,
    pub post_hook: Option<String>,
    pub finally_hook: Option<String>,
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
    let original_value = ctx.task.value.clone();

    let effective_value = match run_pre_hook(ctx.pre_hook.as_ref(), &original_value) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(InFlightResult {
                task: ctx.task,
                task_id: ctx.task_id,
                origin_id: ctx.origin_id,
                step_name: ctx.step_name,
                effective_value: original_value,
                result: SubmitResult::PreHookError(e),
                post_hook: ctx.post_hook,
                finally_hook: ctx.finally_hook,
            });
            return;
        }
    };

    let payload = build_agent_payload(&ctx.step_name, &effective_value, docs, timeout);
    debug!(payload = %payload, "task payload");

    let result = submit_via_cli(pool_root, &payload, invoker);
    let _ = tx.send(InFlightResult {
        task: ctx.task,
        task_id: ctx.task_id,
        origin_id: ctx.origin_id,
        step_name: ctx.step_name,
        effective_value,
        result: SubmitResult::Pool(result),
        post_hook: ctx.post_hook,
        finally_hook: ctx.finally_hook,
    });
}

/// Execute a command task (runs in spawned thread).
pub fn dispatch_command_task(
    ctx: TaskContext,
    script: &str,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let original_value = ctx.task.value.clone();
    let task_step = ctx.task.step.clone();

    let effective_value = match run_pre_hook(ctx.pre_hook.as_ref(), &original_value) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(InFlightResult {
                task: ctx.task,
                task_id: ctx.task_id,
                origin_id: ctx.origin_id,
                step_name: ctx.step_name,
                effective_value: original_value,
                result: SubmitResult::PreHookError(e),
                post_hook: ctx.post_hook,
                finally_hook: ctx.finally_hook,
            });
            return;
        }
    };

    let task_json = serde_json::to_string(&serde_json::json!({
        "kind": task_step,
        "value": effective_value,
    }))
    .unwrap_or_default();

    let result: io::Result<String> = run_command_action(script, &task_json, working_dir);
    let _ = tx.send(InFlightResult {
        task: ctx.task,
        task_id: ctx.task_id,
        origin_id: ctx.origin_id,
        step_name: ctx.step_name,
        effective_value,
        result: SubmitResult::Command(result),
        post_hook: ctx.post_hook,
        finally_hook: ctx.finally_hook,
    });
}

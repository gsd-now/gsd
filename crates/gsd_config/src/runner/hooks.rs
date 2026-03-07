//! Shell hook execution (pre, post, command actions).

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, info};

use crate::types::HookScript;

use super::PostHookInput;

/// Run a shell command with stdin input and capture output.
///
/// Returns the stdout on success, or an error message on failure.
fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
) -> Result<String, String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe - command may exit without reading stdin
        let _ = stdin.write_all(stdin_input.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "exited with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("output is not valid UTF-8: {e}"))
}

/// Run a pre hook and return the (possibly modified) value.
pub fn run_pre_hook(
    script: &HookScript,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    info!(script = %script, "running pre hook");

    let input = serde_json::to_string(value).unwrap_or_default();
    let stdout =
        run_shell_command(script.as_str(), &input, None).map_err(|e| format!("pre hook {e}"))?;

    serde_json::from_str(&stdout)
        .map_err(|e| format!("pre hook output is not valid JSON: {e}"))
        .inspect(|_| {
            debug!("pre hook transformed value");
        })
}

/// Run a post hook synchronously and return the (possibly modified) result.
///
/// Post hooks can modify the `next` array to filter, add, or transform tasks.
pub fn run_post_hook(script: &HookScript, input: &PostHookInput) -> Result<PostHookInput, String> {
    info!(script = %script, kind = ?std::mem::discriminant(input), "running post hook");

    let input_json = serde_json::to_string(&input).unwrap_or_default();
    let stdout = run_shell_command(script.as_str(), &input_json, None)
        .map_err(|e| format!("post hook {e}"))?;

    serde_json::from_str(&stdout)
        .map_err(|e| format!("post hook output is not valid JSON: {e}"))
        .inspect(|_| {
            debug!(script = %script.as_str(), "post hook completed");
        })
}

/// Call a wake script before starting the runner.
pub fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "[E019] wake script failed with status: {status}"
        )))
    }
}

/// Run a command action (shell script) with task JSON on stdin.
pub fn run_command_action(script: &str, task_json: &str, working_dir: &Path) -> io::Result<String> {
    run_shell_command(script, task_json, Some(working_dir))
        .map_err(|e| io::Error::other(format!("[E021] command {e}")))
}

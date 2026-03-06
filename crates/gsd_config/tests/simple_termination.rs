//! Simplest test: a single step that immediately terminates.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, Task};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "simple_termination";

fn simple_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": {"inline": "You are at the start. Return an empty array to finish."}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn single_step_terminates() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::terminator(&root, Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
        invoker: &invoker,
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    // Verify the payload contained the task
    let payload: serde_json::Value = serde_json::from_str(&processed[0]).expect("parse payload");
    assert_eq!(payload["task"]["kind"], "Start");

    cleanup_test_dir(TEST_DIR);
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn empty_initial_tasks_does_nothing() {
    let root = setup_test_dir(&format!("{TEST_DIR}_empty"));

    // No IPC needed - we're not even starting the pool
    // With no initial tasks, the runner completes immediately without connecting to the pool
    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![],
        invoker: &invoker,
    };

    // Should complete immediately without error
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_empty"));
}

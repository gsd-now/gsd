//! Test corresponding to demos/greeting.sh
//! Greeting agent with casual and formal styles.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, SubmitMode, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir,
    submit_with_mode,
};
use rstest::rstest;
use std::time::Duration;

const TEST_DIR: &str = "greeting";

#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn greeting_casual_and_formal(#[case] mode: SubmitMode) {
    // Use mode in test dir name to avoid conflicts when tests run in parallel
    let test_dir = format!("{TEST_DIR}_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let casual = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#,
        mode,
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = casual else {
        panic!("Expected Processed response, got {casual:?}");
    };
    assert_eq!(stdout.trim(), "Hi friendly-bot, how are ya?");

    let formal = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"formal"}}"#,
        mode,
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = formal else {
        panic!("Expected Processed response, got {formal:?}");
    };
    assert_eq!(
        stdout.trim(),
        "Salutations friendly-bot, how are you doing on this most splendiferous and utterly magnificent day?"
    );

    // Note: processed contains the full task JSON
    let _ = agent.stop();

    cleanup_test_dir(&test_dir);
}

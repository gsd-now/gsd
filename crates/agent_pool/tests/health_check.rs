//! Tests for health check functionality.
//!
//! Tests the state machine:
//! - Initial health check on registration
//! - Periodic health checks to idle agents
//! - Timeout handling for unresponsive agents
//! - Agents can recover after timeout by re-registering

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::{AGENTS_DIR, DaemonConfig, Response};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::fs;
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "health_check";

/// Test that initial health check is sent on registration when enabled.
#[test]
fn initial_health_check_on_registration() {
    let root = setup_test_dir(&format!("{TEST_DIR}_initial"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_initial"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: true,
        periodic_health_check: false,
        health_check_interval: Duration::from_secs(60),
        health_check_timeout: Duration::from_secs(30),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);
    let agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Give time for registration and health check
    thread::sleep(Duration::from_millis(300));

    // Submit a real task - should work since agent responded to health check
    let response = agent_pool::submit(&root, "Test task").expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Test task [processed]");

    // Agent should have processed the real task (health checks not counted)
    let processed = agent.stop();
    assert_eq!(processed, vec!["Test task"]);

    cleanup_test_dir(&format!("{TEST_DIR}_initial"));
}

/// Test that no initial health check is sent when disabled.
#[test]
fn no_initial_health_check_when_disabled() {
    let root = setup_test_dir(&format!("{TEST_DIR}_no_initial"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_no_initial"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: false,
        periodic_health_check: false,
        health_check_interval: Duration::from_secs(60),
        health_check_timeout: Duration::from_secs(30),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);
    let agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Give time for registration
    thread::sleep(Duration::from_millis(200));

    // Submit task immediately - should work without waiting for health check
    let response = agent_pool::submit(&root, "Test task").expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Test task [processed]");

    let processed = agent.stop();
    assert_eq!(processed, vec!["Test task"]);

    cleanup_test_dir(&format!("{TEST_DIR}_no_initial"));
}

/// Test that health check timeout deregisters the agent.
#[test]
fn health_check_timeout_deregisters_agent() {
    let root = setup_test_dir(&format!("{TEST_DIR}_timeout"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_timeout"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: true,
        periodic_health_check: false,
        health_check_interval: Duration::from_secs(60),
        // Very short timeout for testing
        health_check_timeout: Duration::from_millis(100),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);

    // Create agent directory but DON'T start the agent (won't respond to health check)
    let agent_dir = root.join(AGENTS_DIR).join("unresponsive-agent");
    fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

    // Wait for initial health check to be sent and timeout
    thread::sleep(Duration::from_millis(800));

    // Agent directory should be removed due to timeout
    assert!(
        !agent_dir.exists(),
        "Agent directory should be removed after health check timeout"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_timeout"));
}

/// Test that agent can recover after timeout by re-registering.
#[test]
fn agent_recovery_after_timeout() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_recovery"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: true,
        periodic_health_check: false,
        health_check_interval: Duration::from_secs(60),
        health_check_timeout: Duration::from_millis(100),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);

    // Create agent directory but don't respond (simulate unresponsive agent)
    let agent_dir = root.join(AGENTS_DIR).join("recovering-agent");
    fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

    // Wait for timeout
    thread::sleep(Duration::from_millis(800));

    // Agent should be deregistered
    assert!(
        !agent_dir.exists(),
        "Agent should be deregistered after timeout"
    );

    // Now start a real agent with the same name - should re-register
    let agent = TestAgent::echo(&root, "recovering-agent", Duration::from_millis(10));

    // Wait for registration and health check
    thread::sleep(Duration::from_millis(300));

    // Submit task - should work
    let response = agent_pool::submit(&root, "Recovery test").expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Recovery test [processed]");

    let processed = agent.stop();
    assert_eq!(processed, vec!["Recovery test"]);

    cleanup_test_dir(&format!("{TEST_DIR}_recovery"));
}

/// Test that periodic health checks are sent to idle agents.
#[test]
fn periodic_health_check_to_idle_agent() {
    let root = setup_test_dir(&format!("{TEST_DIR}_periodic"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_periodic"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: false,
        periodic_health_check: true,
        // Very short interval for testing
        health_check_interval: Duration::from_millis(200),
        health_check_timeout: Duration::from_secs(30),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);
    let agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Wait for registration
    thread::sleep(Duration::from_millis(100));

    // Submit initial task
    let response = agent_pool::submit(&root, "Task 1").expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Task 1 [processed]");

    // Wait long enough for periodic health check to be sent (agent is now idle)
    thread::sleep(Duration::from_millis(500));

    // Submit another task - should still work (agent passed health check)
    let response = agent_pool::submit(&root, "Task 2").expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Task 2 [processed]");

    // Agent should have processed both real tasks (health checks not counted)
    let processed = agent.stop();
    assert_eq!(processed, vec!["Task 1", "Task 2"]);

    cleanup_test_dir(&format!("{TEST_DIR}_periodic"));
}

/// Test that busy agents don't receive periodic health checks.
#[test]
fn no_periodic_health_check_to_busy_agent() {
    let root = setup_test_dir(&format!("{TEST_DIR}_busy"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_busy"));
        return;
    }

    let config = DaemonConfig {
        initial_health_check: false,
        periodic_health_check: true,
        // Very short interval
        health_check_interval: Duration::from_millis(100),
        health_check_timeout: Duration::from_secs(30),
    };

    let _pool = AgentPoolHandle::start_with_config(&root, config);

    // Start agent with slow processing to keep it busy
    let agent = TestAgent::echo(&root, "slow-agent", Duration::from_millis(500));

    thread::sleep(Duration::from_millis(100));

    // Submit task - agent will be busy processing for 500ms
    let handle = thread::spawn(move || agent_pool::submit(&root, "Slow task"));

    // Wait a bit - agent should be busy, no health check should be sent
    thread::sleep(Duration::from_millis(200));

    // Wait for task to complete
    let response = handle
        .join()
        .expect("Thread panicked")
        .expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Slow task [processed]");

    // Agent should only have processed the one real task
    let processed = agent.stop();
    assert_eq!(processed, vec!["Slow task"]);

    cleanup_test_dir(&format!("{TEST_DIR}_busy"));
}

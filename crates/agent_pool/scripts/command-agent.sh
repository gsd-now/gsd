#!/bin/bash
# Agent that executes shell commands from tasks.
#
# Usage: ./command-agent.sh --pool <POOL_ID> --name <AGENT_NAME>
#
# This agent:
# 1. Connects to the pool and waits for tasks
# 2. Extracts the "cmd" field from the task value
# 3. Executes it with bash
# 4. Writes stdout to the response file
# 5. Loops for the next task
#
# Task format expected:
#   {"task": {"kind": "...", "value": {"cmd": "cargo test"}}, "instructions": "..."}
#
# Response format:
#   The raw stdout from the command (agent pool wraps this in {"kind":"Processed","stdout":"..."})

set -e

# Parse arguments
POOL=""
NAME=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --pool) POOL="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ -z "$POOL" ] || [ -z "$NAME" ]; then
    echo "Usage: $0 --pool <POOL_ID> --name <AGENT_NAME>" >&2
    exit 1
fi

# Use agent_pool from PATH, or override with AGENT_POOL env var
AGENT_POOL="${AGENT_POOL:-agent_pool}"

echo "[$NAME] Agent started, connected to pool $POOL" >&2

cleanup() {
    echo "[$NAME] Deregistering and shutting down..." >&2
    "$AGENT_POOL" deregister_agent --pool "$POOL" --name "$NAME" 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

while true; do
    # Wait for a task
    echo "[$NAME] Waiting for task..." >&2
    TASK_JSON=$("$AGENT_POOL" get_task --pool "$POOL" --name "$NAME" 2>/dev/null)

    # Extract response file path and command
    RESPONSE_FILE=$(echo "$TASK_JSON" | jq -r '.response_file')
    CMD=$(echo "$TASK_JSON" | jq -r '.content.task.value.cmd // .content.cmd // empty')

    if [ -z "$CMD" ]; then
        echo "[$NAME] No 'cmd' field in task, skipping" >&2
        echo "[]" > "$RESPONSE_FILE"
        continue
    fi

    echo "[$NAME] Executing: $CMD" >&2

    # Execute the command and capture output
    set +e
    OUTPUT=$(bash -c "$CMD" 2>&1)
    EXIT_CODE=$?
    set -e

    if [ $EXIT_CODE -eq 0 ]; then
        echo "[$NAME] Command succeeded" >&2
    else
        echo "[$NAME] Command failed with exit code $EXIT_CODE" >&2
    fi

    # Write response (just the output - agent_pool wraps it)
    echo "$OUTPUT" > "$RESPONSE_FILE"
done

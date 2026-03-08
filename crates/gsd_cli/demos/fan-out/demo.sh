#!/bin/bash
# Demo: Fan-out with multiple agents (Distribute -> Worker x20)
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

EXISTING_POOL="$1"
WAKE_SCRIPT="$2"

cargo build -p agent_pool -p gsd_cli --quiet

export AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"
export GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

NUM_WORKERS=20
NUM_AGENTS=3
WORKER_SLEEP=0.3

if [ -n "$EXISTING_POOL" ]; then
    WAKE_ARG=""
    if [ -n "$WAKE_SCRIPT" ]; then
        WAKE_ARG="--wake $WAKE_SCRIPT"
    fi

    $GSD run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$EXISTING_POOL" \
        --initial-state '[{"kind": "Distribute", "value": {}}]' \
        $WAKE_ARG
else
    POOL_ROOT=$(mktemp -d)
    POOL_ID="demo"
    AGENT_PIDS=""

    cleanup() {
        $AGENT_POOL --root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
        sleep 0.2
        for pid in $AGENT_PIDS; do
            kill -9 $pid 2>/dev/null || true
        done
        for pid in $AGENT_PIDS; do
            wait $pid 2>/dev/null || true
        done
        rm -rf "$POOL_ROOT"
    }
    trap cleanup EXIT

    $AGENT_POOL --root "$POOL_ROOT" start --pool "$POOL_ID" --log-level "${LOG_LEVEL:-warn}" &
    POOL_PID=$!
    sleep 0.5

    for i in $(seq 1 $NUM_AGENTS); do
        "$SCRIPT_DIR/../../scripts/fan-out-agent.sh" --root "$POOL_ROOT" --pool "$POOL_ID" --name "agent-$i" --workers "$NUM_WORKERS" --sleep "$WORKER_SLEEP" &
        AGENT_PIDS="$AGENT_PIDS $!"
    done
    sleep 0.3

    $GSD --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$POOL_ID" \
        --initial-state '[{"kind": "Distribute", "value": {}}]'
fi

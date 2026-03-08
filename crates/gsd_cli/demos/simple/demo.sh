#!/bin/bash
# Demo: Simple single-step GSD task queue
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

EXISTING_POOL="$1"
WAKE_SCRIPT="$2"

cargo build -p agent_pool -p gsd_cli --quiet

export AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"
export GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

if [ -n "$EXISTING_POOL" ]; then
    WAKE_ARG=""
    if [ -n "$WAKE_SCRIPT" ]; then
        WAKE_ARG="--wake $WAKE_SCRIPT"
    fi

    $GSD run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$EXISTING_POOL" \
        --initial-state '[{"kind": "Start", "value": {}}]' \
        $WAKE_ARG
else
    POOL_ROOT=$(mktemp -d)
    POOL_ID="demo"

    cleanup() {
        $AGENT_POOL --root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
        sleep 0.2
        kill -9 $AGENT_PID 2>/dev/null || true
        wait $AGENT_PID 2>/dev/null || true
        rm -rf "$POOL_ROOT"
    }
    trap cleanup EXIT

    $AGENT_POOL --root "$POOL_ROOT" start --pool "$POOL_ID" --log-level "${LOG_LEVEL:-info}" &
    POOL_PID=$!
    sleep 0.5

    "$SCRIPT_DIR/../../scripts/gsd-agent.sh" --root "$POOL_ROOT" --pool "$POOL_ID" --name "gsd-agent-1" --sleep 0.1 &
    AGENT_PID=$!
    sleep 0.3

    $GSD --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$POOL_ID" \
        --initial-state '[{"kind": "Start", "value": {}}]'
fi

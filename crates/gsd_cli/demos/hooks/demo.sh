#!/bin/bash
# Demo: Pre-hook, post-hook, and finally-hook execution
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

cargo build -p gsd_cli --quiet

export GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

cleanup() {
    rm -rf "$POOL_ROOT"
}
trap cleanup EXIT

$GSD --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --initial-state '[{"kind": "Process", "value": {"item": "test-item"}}]'

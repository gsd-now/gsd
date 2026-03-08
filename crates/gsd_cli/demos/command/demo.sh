#!/bin/bash
# Demo: Command actions (Split -> Process x3 -> Collect x3)
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

cargo build -p gsd_cli --quiet

GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

POOL_ROOT=$(mktemp -d)
POOL_ID="demo"
cleanup() {
    rm -rf "$POOL_ROOT"
}
trap cleanup EXIT

$GSD --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --initial-state '[{"kind": "Split", "value": {"items": [{"n": 1}, {"n": 2}, {"n": 3}]}}]'

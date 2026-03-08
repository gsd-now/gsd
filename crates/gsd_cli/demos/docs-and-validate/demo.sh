#!/bin/bash
# Demo: GSD config docs and validate commands
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

cargo build -p gsd_cli --quiet

GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

$GSD config validate --config "$SCRIPT_DIR/../simple/config.jsonc"
$GSD config validate --config "$SCRIPT_DIR/../linear/config.jsonc"
$GSD config validate --config "$SCRIPT_DIR/../branching/config.jsonc"
$GSD config docs --config "$SCRIPT_DIR/../linear/config.jsonc"

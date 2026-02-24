#!/bin/bash
# Start 5 command agents for the cmd pool.
#
# Usage: ./scripts/start-cmd-agents.sh [num-agents]

set -e

cd "$(dirname "$0")/.."

NUM_AGENTS="${1:-5}"

> /tmp/agent.log

# Start N-1 agents in background, last one in foreground
for i in $(seq 1 $((NUM_AGENTS - 1))); do
    ./crates/agent_pool/scripts/command-agent.sh --pool cmd 2>&1 | tee -a /tmp/agent.log &
done

# Last agent runs in foreground (keeps script alive)
./crates/agent_pool/scripts/command-agent.sh --pool cmd 2>&1 | tee -a /tmp/agent.log

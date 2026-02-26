#!/bin/bash
set -e

GSD=$(pnpm view @gsd-now/gsd dist-tags.main 2>/dev/null)
AGENT_POOL=$(pnpm view @gsd-now/agent-pool dist-tags.main 2>/dev/null)

echo "$GSD"
echo ""
echo "pnpm install @gsd-now/gsd@$GSD"
echo "pnpm install @gsd-now/agent-pool@$AGENT_POOL"

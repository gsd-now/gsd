# Finally Tracking Refactor

**Status:** Not started

**Blocks:** STATE_PERSISTENCE

## Motivation

The current finally tracking algorithm uses a flat structure that can't be reconstructed from the state log. We need a tree-based approach where each task tracks its own children.

## Current Algorithm

- `origin_id` on each task points directly to the ancestor with finally hook (skips levels)
- `finally_tracking: HashMap<u64, FinallyState>` keyed by that ancestor
- When task completes, decrement the ancestor's counter directly

Problem: On resume, we can't reconstruct `finally_tracking` because `origin_id` skips intermediate tasks.

## Proposed Algorithm

Each task tracks its own spawned children count. State propagates up the tree level by level.

### Task State

```rust
enum TaskState {
    Pending,                  // Waiting for agent
    AwaitingDescendants(u32), // Agent done, N spawned tasks still completing
}
```

### Data Changes

- `origin_id` becomes immediate parent only (not finally-ancestor)
- Remove `finally_tracking` HashMap
- Add per-task state tracking

### Flow

1. **TaskSubmitted**: task is `Pending`

2. **Agent completes (success)**:
   - If spawned > 0: transition to `AwaitingDescendants(spawned.len())`
   - If spawned == 0: task is fully done → propagate up

3. **Propagate up** (when task fully done):
   - Run this task's finally hook (if any)
   - Decrement parent's `AwaitingDescendants` count
   - If parent's count hits 0: recurse (propagate up from parent)

### Example

```
A (finally hook) spawns B, C
B spawns D

Initial:
  A: AwaitingDescendants(2)  # B, C
  B: AwaitingDescendants(1)  # D
  C: Pending
  D: Pending

D completes (no children):
  D fully done
  B: AwaitingDescendants(0) → fully done → run B's finally (none)
  A: AwaitingDescendants(1)  # just C now

C completes (no children):
  C fully done
  A: AwaitingDescendants(0) → fully done → run A's finally hook
```

## Why This Enables Resume

On resume, we can reconstruct:
- Pending tasks from TaskSubmitted without TaskCompleted
- Parent-child relationships from `origin_id` (immediate parent)
- `AwaitingDescendants` counts by counting pending children

No special `finally_tracking` state needed - it's derived from the tree structure.

## Implementation

1. Change `QueuedTask.origin_id` to always be immediate parent
2. Add `TaskState` enum to track awaiting descendants
3. Replace `finally_tracking` HashMap with per-task state
4. Update completion logic to propagate up tree

## Files Changed

- `crates/gsd_config/src/runner.rs`

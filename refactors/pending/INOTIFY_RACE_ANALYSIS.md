# Inotify Race Condition Analysis

## The Problem

Tests pass on macOS but fail (hang) on Linux. The root cause is a race condition inherent to `inotify` that doesn't exist in `FSEvents`.

### FSEvents vs Inotify

**macOS FSEvents:**
- Directory-level monitoring
- Watches entire tree automatically
- No race when subdirectories are created

**Linux inotify:**
- Per-directory watches
- Must manually add watches for new subdirectories
- **Race condition**: When a new directory is created, there's a window between:
  1. Receiving the CREATE event for the directory
  2. Adding a watch for that directory
- Files written during this window are missed

## Where The Race Occurs

### 1. Pending Task Submission (`NotifyMethod::Raw`)

```
1. Submitter creates `pending/<uuid>/`
2. inotify receives CREATE event
3. notify crate tries to add watch for `pending/<uuid>/`
4. Submitter writes `task.json`
5. If (4) happens before (3), we miss the PendingTask event
```

**Fix added:** `PendingDir` fallback in `wiring.rs:557-571`

When we see a `PendingDir` event, check if `task.json` already exists and register it immediately.

### 2. Agent Response Detection

```
1. Agent creates `agents/<name>/`
2. inotify receives CREATE event
3. notify crate tries to add watch for `agents/<name>/`
4. Daemon writes `task.json` (heartbeat)
5. Agent reads `task.json`, writes `response.json`
6. If (5) happens before (3), we miss the AgentResponse event
```

**Fix added:** `AgentDir` fallback in `wiring.rs:622-636`

When we see an `AgentDir` event and register a new agent, check if `response.json` already exists and trigger `AgentResponded` if so.

## Structural Improvements

These are architectural changes that would eliminate the race condition entirely, not just mitigate it.

### Option A: Flatten Directory Structure

Currently: `pending/<uuid>/task.json` and `pending/<uuid>/response.json`

Proposed: `pending/<uuid>.task.json` and `pending/<uuid>.response.json`

This eliminates the subdirectory creation, removing the race. The daemon would watch `pending/` directly for files matching `*.task.json`.

**Pros:**
- Eliminates race entirely
- Simpler directory structure
- Fewer inotify watches needed

**Cons:**
- Breaking change to protocol
- Can't easily clean up both task and response together

### Option B: Explicit Synchronization

Add a sync file that must exist before task.json:

```
1. Submitter creates `pending/<uuid>/`
2. Submitter creates `pending/<uuid>/.ready`
3. Daemon sees .ready, confirms watch is active
4. Daemon creates `pending/<uuid>/.ack`
5. Submitter sees .ack, writes `task.json`
```

**Pros:**
- Explicit handshake ensures no race
- Backward compatible (old clients just skip the sync)

**Cons:**
- Adds latency
- More complex protocol

### Option C: Use Socket Notification Always

For `NotifyMethod::Raw`, after writing `task.json`, the submitter could:
1. Connect to daemon socket
2. Send a "PollPending" message with the UUID
3. Daemon explicitly scans that directory

**Pros:**
- Eliminates reliance on FS events for submission
- Fast and reliable

**Cons:**
- Requires socket access (may not work in all sandboxes)
- Hybrid approach is more complex

### Option D: Periodic Scanning (NOT RECOMMENDED)

Add a background thread that periodically scans `pending/` and `agents/` directories.

**Pros:**
- Simple to implement
- Catches any missed events

**Cons:**
- Adds latency (up to scan interval)
- Wasteful on resources
- Masks the underlying problem
- **Not a good solution** - should only be used for debugging

## Current State

The fallback checks in `handle_pending_dir` and `handle_agent_dir` should handle most race conditions. However, they are mitigations, not structural fixes.

**To verify the fix works on Linux:**
1. Fix GitHub Actions billing
2. Run CI on the `fix-linux-tests` branch
3. If tests still fail, add more diagnostic logging to identify exactly which event is being missed

## Investigation Tests

Added `inotify_investigation.rs` with three tests:
- `minimal_socket_test`: Tests agent response detection only
- `minimal_raw_test`: Tests both task submission and response detection
- `rapid_submissions_test`: Stress test for race conditions

If `minimal_socket_test` passes but `minimal_raw_test` fails, the issue is with task submission detection.
If both fail, the issue is with agent response detection.

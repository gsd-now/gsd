# Cleanup: Misplaced Code

## 1. ✅ `stop` moved out of `submit/`

**DONE**: Moved from `crates/agent_pool/src/submit/stop.rs` to `crates/agent_pool/src/stop.rs`.

## 2. ❌ `resolve_pool` - NOT dead code

**REVERTED**: The slash check is NOT dead code. Tests and users pass full paths to `--pool`, which `resolve_pool` must handle. The original logic that accepts both IDs and paths is correct.

## 3. ✅ String literals replaced with constants

**DONE**: Added `STATUS_READY` and `STATUS_STOP` constants to `constants.rs`. Updated all usages in:
- `crates/agent_pool/src/stop.rs` - uses `STATUS_STOP`
- `crates/agent_pool/src/daemon/wiring.rs` - uses `STATUS_READY`
- `crates/agent_pool/src/daemon/path_category.rs` - uses `STATUS_STOP`

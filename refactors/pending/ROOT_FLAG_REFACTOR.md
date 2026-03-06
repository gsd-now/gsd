# Root Flag Refactor

**Status:** Not started

**Dependency:** STATE_PERSISTENCE depends on this (state files will live under `--root`)

## Motivation

Currently `--pool-root` points directly to the parent of pool directories:
```
--pool-root /tmp/agent_pool --pool mypool
→ /tmp/agent_pool/mypool/
```

We want a cleaner `--root` that contains a `pools/` subfolder implicitly:
```
--root /tmp/gsd --pool mypool
→ /tmp/gsd/pools/mypool/
```

This allows `--root` to contain other things in the future (state files, logs, etc.) without polluting the pool namespace.

## Current State

### agent_pool

```rust
// crates/agent_pool/src/pool.rs
pub fn default_pool_root() -> PathBuf {
    std::env::temp_dir().join("agent_pool")  // /tmp/agent_pool
}

pub fn resolve_pool(pool_root: &Path, reference: &str) -> PathBuf {
    pool_root.join(reference)  // /tmp/agent_pool/<pool>
}
```

### CLI flags

```rust
// Both gsd and agent_pool CLIs
#[arg(long, global = true)]
pool_root: Option<PathBuf>,
```

## Proposed Changes

### 1. Rename `--pool-root` to `--root`

Both `gsd` and `agent_pool` CLIs:
```rust
#[arg(long, global = true)]
root: Option<PathBuf>,
```

### 2. Add implicit `pools/` subdirectory

```rust
// crates/agent_pool/src/pool.rs

const POOLS_DIR: &str = "pools";

pub fn default_root() -> PathBuf {
    std::env::temp_dir().join("agent_pool")  // /tmp/agent_pool
}

pub fn pools_dir(root: &Path) -> PathBuf {
    root.join(POOLS_DIR)  // /tmp/agent_pool/pools
}

pub fn resolve_pool(root: &Path, pool_id: &str) -> PathBuf {
    pools_dir(root).join(pool_id)  // /tmp/agent_pool/pools/<pool>
}
```

### 3. Update all callers

Every place that constructs pool paths needs to go through `resolve_pool()` or `pools_dir()`.

## Migration

The default root stays the same (`/tmp/agent_pool`), but pools now live under `/tmp/agent_pool/pools/` instead of `/tmp/agent_pool/`.

This is a breaking change for anyone with existing pools, but:
- Pools are ephemeral (no persistent data)
- Users just need to restart their daemons

## Files to Change

| File | Changes |
|------|---------|
| `crates/agent_pool/src/pool.rs` | Add `POOLS_DIR`, update `resolve_pool()` |
| `crates/agent_pool/src/lib.rs` | Export new functions |
| `crates/agent_pool_cli/src/main.rs` | Rename `--pool-root` to `--root` |
| `crates/gsd_cli/src/main.rs` | Rename `--pool-root` to `--root` |
| All tests | Update pool path expectations |
| README.md, docs | Update examples |

## Example

Before:
```bash
agent_pool start --pool-root /tmp/myroot --pool work
# Pool at: /tmp/myroot/work/

gsd run config.jsonc --pool-root /tmp/myroot --pool work
```

After:
```bash
agent_pool start --root /tmp/myroot --pool work
# Pool at: /tmp/myroot/pools/work/

gsd run config.jsonc --root /tmp/myroot --pool work
```

Users never write "pools" - it's implicit in `resolve_pool()`.

## Future

With `--root` containing a `pools/` subdirectory, we can add:
- `--root/state/` for state files (STATE_PERSISTENCE)
- `--root/logs/` for log files
- `--root/config/` for cached configs

All without conflicting with pool names.

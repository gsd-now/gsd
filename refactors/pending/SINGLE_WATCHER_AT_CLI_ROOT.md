# Single Watcher Created at CLI Root

**Depends on:** `WAIT_FOR_POOL_READY_WATCHER.md`

## Motivation

Currently, various functions create their own `VerifiedWatcher` internally:
- `wait_for_pool_ready` creates one
- `wait_for_task` creates one
- `submit_file` creates one

This is wasteful and makes it hard to reason about resource usage. We want exactly one watcher per CLI invocation, created at the top level and threaded down.

## Goal

Create the watcher at CLI entry point (`main()`) and pass it to all functions that need it. No function should create a watcher internally.

## Affected CLIs

1. **agent_pool** - `crates/agent_pool_cli/src/main.rs`
2. **gsd** - `crates/gsd_cli/src/main.rs`

## Implementation

### PoolWatcher wrapper

```rust
// crates/agent_pool/src/pool_watcher.rs

pub struct PoolWatcher {
    inner: VerifiedWatcher,
    pool_root: PathBuf,
}

impl PoolWatcher {
    /// Create a watcher for all pool directories.
    pub fn new(pool_root: &Path) -> io::Result<Self> {
        let agents_dir = pool_root.join(AGENTS_DIR);
        let submissions_dir = pool_root.join(SUBMISSIONS_DIR);

        // Ensure directories exist (daemon may not have created them yet)
        // We watch parent directories that do exist
        let mut watch_dirs = vec![pool_root.to_path_buf()];
        if agents_dir.exists() {
            watch_dirs.push(agents_dir);
        }
        if submissions_dir.exists() {
            watch_dirs.push(submissions_dir);
        }

        let watch_refs: Vec<&Path> = watch_dirs.iter().map(|p| p.as_path()).collect();
        let inner = VerifiedWatcher::new(pool_root, &watch_refs)?;

        Ok(Self {
            inner,
            pool_root: pool_root.to_path_buf(),
        })
    }

    pub fn wait_for_file(&mut self, target: &Path) -> io::Result<()> {
        self.inner.wait_for_file(target)
    }

    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> io::Result<()> {
        self.inner.wait_for_file_with_timeout(target, timeout)
    }

    pub fn pool_root(&self) -> &Path {
        &self.pool_root
    }
}
```

### Update function signatures

All functions that currently create watchers internally now take `&mut PoolWatcher`:

```rust
// Before
pub fn wait_for_pool_ready(root: &Path, timeout: Duration) -> io::Result<()>;
pub fn wait_for_task(pool_root: &Path, name: Option<&str>) -> io::Result<TaskAssignment>;
pub fn submit_file(root: &Path, payload: &Payload) -> io::Result<Response>;

// After
pub fn wait_for_pool_ready(watcher: &mut PoolWatcher, timeout: Duration) -> io::Result<()>;
pub fn wait_for_task(watcher: &mut PoolWatcher, name: Option<&str>) -> io::Result<TaskAssignment>;
pub fn submit_file(watcher: &mut PoolWatcher, payload: &Payload) -> io::Result<Response>;
```

### agent_pool CLI

```rust
// crates/agent_pool_cli/src/main.rs

fn main() -> ExitCode {
    match cli.command {
        Command::GetTask { pool, name } => {
            let root = resolve_pool(&pool_root, &pool);

            // Create watcher at CLI root
            let mut watcher = match PoolWatcher::new(&root) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Wait for daemon ready
            if let Err(e) = wait_for_pool_ready(&mut watcher, Duration::from_secs(5)) {
                eprintln!("Daemon not ready: {e}");
                return ExitCode::FAILURE;
            }

            // Get task
            match wait_for_task(&mut watcher, name.as_deref()) {
                Ok(assignment) => { /* ... */ }
                Err(e) => { /* ... */ }
            }
        }
        // ... other commands
    }
}
```

### gsd CLI

```rust
// crates/gsd_cli/src/main.rs

fn main() -> io::Result<()> {
    match cli.command {
        Command::Run { pool, ... } => {
            let pool_path = resolve_pool(...);

            // Create watcher at CLI root
            let mut watcher = PoolWatcher::new(&pool_path)?;

            // Pass to runner
            let runner_config = RunnerConfig {
                watcher: &mut watcher,
                // ...
            };

            run(&cfg, &schemas, runner_config)?;
        }
    }
}
```

### RunnerConfig changes

```rust
pub struct RunnerConfig<'a> {
    pub watcher: &'a mut PoolWatcher,
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub agent_pool_binary: Option<&'a Path>,
}
```

## Migration Steps

1. Create `PoolWatcher` struct in `crates/agent_pool/src/pool_watcher.rs`
2. Export from `lib.rs`
3. Update `wait_for_pool_ready` signature to take `&mut PoolWatcher`
4. Update `wait_for_task` signature to take `&mut PoolWatcher`
5. Update `submit_file` signature to take `&mut PoolWatcher`
6. Update agent_pool CLI to create watcher in `main()` for `GetTask` command
7. Update gsd CLI to create watcher in `main()` for `Run` command
8. Update `RunnerConfig` to include watcher reference
9. Update all internal call sites to use passed watcher
10. Remove any internal watcher creation from library functions
11. Update tests to create and pass watchers

## Testing

- All existing tests pass
- CLI commands work correctly
- No watcher created except at CLI root

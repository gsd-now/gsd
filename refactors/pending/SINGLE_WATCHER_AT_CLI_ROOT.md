# Single Watcher Created at CLI Root

**Depends on:** `WAIT_FOR_POOL_READY_WATCHER.md`

## Motivation

Currently, various functions create their own `VerifiedWatcher` internally:
- `wait_for_pool_ready` creates one
- `wait_for_task` creates one
- `submit_file` creates one

This is wasteful and makes it hard to reason about resource usage. We want exactly one watcher per CLI invocation, created at the top level and threaded down.

## Goal

Create the `VerifiedWatcher` at CLI entry point (`main()`) and pass it to all functions that need it. No function should create a watcher internally.

## Affected CLIs

1. **agent_pool** - `crates/agent_pool_cli/src/main.rs`
2. **gsd** - `crates/gsd_cli/src/main.rs`

## Implementation

### Create watcher at CLI root

```rust
/// Create a watcher for all pool directories.
fn create_pool_watcher(pool_root: &Path) -> io::Result<VerifiedWatcher> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let submissions_dir = pool_root.join(SUBMISSIONS_DIR);

    // Watch all directories we'll need
    let watch_dirs = [pool_root, &agents_dir, &submissions_dir];
    VerifiedWatcher::new(pool_root, &watch_dirs)
}
```

### Update function signatures

All functions that currently create watchers internally now take `&mut VerifiedWatcher`:

```rust
// Before
pub fn wait_for_pool_ready(root: &Path, timeout: Duration) -> io::Result<()>;
pub fn wait_for_task(pool_root: &Path, name: Option<&str>) -> io::Result<TaskAssignment>;
pub fn submit_file(root: &Path, payload: &Payload) -> io::Result<Response>;

// After
pub fn wait_for_pool_ready(watcher: &mut VerifiedWatcher, root: &Path, timeout: Duration) -> io::Result<()>;
pub fn wait_for_task(watcher: &mut VerifiedWatcher, pool_root: &Path, name: Option<&str>) -> io::Result<TaskAssignment>;
pub fn submit_file(watcher: &mut VerifiedWatcher, root: &Path, payload: &Payload) -> io::Result<Response>;
```

### agent_pool CLI

```rust
// crates/agent_pool_cli/src/main.rs

fn main() -> ExitCode {
    match cli.command {
        Command::GetTask { pool, name } => {
            let root = resolve_pool(&pool_root, &pool);

            // Create watcher at CLI root
            let mut watcher = match create_pool_watcher(&root) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Wait for daemon ready
            if let Err(e) = wait_for_pool_ready(&mut watcher, &root, Duration::from_secs(5)) {
                eprintln!("Daemon not ready: {e}");
                return ExitCode::FAILURE;
            }

            // Get task
            match wait_for_task(&mut watcher, &root, name.as_deref()) {
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
            let mut watcher = create_pool_watcher(&pool_path)?;

            // Pass to runner
            let runner_config = RunnerConfig {
                watcher: &mut watcher,
                pool_root: &pool_path,
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
    pub watcher: &'a mut VerifiedWatcher,
    pub pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub agent_pool_binary: Option<&'a Path>,
}
```

## Migration Steps

1. Add helper `create_pool_watcher(pool_root: &Path) -> io::Result<VerifiedWatcher>`
2. Update `wait_for_pool_ready` signature to take `&mut VerifiedWatcher`
3. Update `wait_for_task` signature to take `&mut VerifiedWatcher`
4. Update `submit_file` signature to take `&mut VerifiedWatcher`
5. Update agent_pool CLI to create watcher in `main()` for `GetTask` command
6. Update gsd CLI to create watcher in `main()` for `Run` command
7. Update `RunnerConfig` to include watcher reference
8. Update all internal call sites to use passed watcher
9. Remove any internal watcher creation from library functions
10. Update tests to create and pass watchers

## Testing

- All existing tests pass
- CLI commands work correctly
- No watcher created except at CLI root

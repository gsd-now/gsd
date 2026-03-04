# Support AGENT_POOL_COMMAND for package manager invocation

## Motivation

Currently, `AGENT_POOL` must point to a binary path. This doesn't work well with package managers like pnpm/npm where users want to run via:

```bash
pnpm dlx agent_pool submit_task ...
npx agent_pool submit_task ...
```

Users must either:
1. Find the binary inside `node_modules/.bin/agent_pool` and point to it
2. Create a wrapper script

This is friction for users who just want to `pnpm add agent_pool` and have it work.

## Current State

### Binary resolution (`gsd_config/src/runner.rs:1016-1024`)

```rust
fn resolve_agent_pool_binary() -> std::path::PathBuf {
    // 1. Environment variable override
    if let Ok(path) = std::env::var("AGENT_POOL") {
        return std::path::PathBuf::from(path);
    }

    // 2. Default: assume it's in PATH
    std::path::PathBuf::from("agent_pool")
}
```

### CLI invocation (`gsd_config/src/runner.rs:977-998`)

```rust
let output = Command::new(&binary)
    .arg("submit_task")
    .arg("--pool-root")
    .arg(pool_root)
    // ... more args
    .output()
    .map_err(|e| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Failed to run agent_pool binary '{}': {e}", binary.display()),
        )
    })?;
```

The code uses `Command::new(binary)` which expects a path, not a shell command with arguments.

## Proposed Solution

Add `AGENT_POOL_COMMAND` environment variable that specifies a command prefix. When set, the system shells out with the full command string.

### Option A: Shell-based invocation

```rust
fn resolve_agent_pool_command() -> AgentPoolInvocation {
    // 1. Command-style invocation (for pnpm dlx, npx, etc.)
    if let Ok(cmd) = std::env::var("AGENT_POOL_COMMAND") {
        return AgentPoolInvocation::Command(cmd);
    }

    // 2. Binary path override
    if let Ok(path) = std::env::var("AGENT_POOL") {
        return AgentPoolInvocation::Binary(PathBuf::from(path));
    }

    // 3. Default: assume it's in PATH
    AgentPoolInvocation::Binary(PathBuf::from("agent_pool"))
}

enum AgentPoolInvocation {
    /// Direct binary path
    Binary(PathBuf),
    /// Shell command (e.g., "pnpm dlx agent_pool")
    Command(String),
}
```

Then in `submit_via_cli`:

```rust
let output = match invocation {
    AgentPoolInvocation::Binary(binary) => {
        Command::new(&binary)
            .arg("submit_task")
            .args(&args)
            .output()
    }
    AgentPoolInvocation::Command(cmd) => {
        // Build full command string
        let full_cmd = format!("{} submit_task {}", cmd, args.join(" "));
        Command::new("sh")
            .arg("-c")
            .arg(&full_cmd)
            .output()
    }
}?;
```

**Pros:**
- Works with any package manager invocation
- Flexible for custom wrappers

**Cons:**
- Shell escaping complexity
- Platform differences (sh vs cmd.exe on Windows)
- Slightly slower due to shell overhead

### Option B: Split command into program + args

```rust
fn resolve_agent_pool_command() -> (PathBuf, Vec<String>) {
    if let Ok(cmd) = std::env::var("AGENT_POOL_COMMAND") {
        // Split "pnpm dlx agent_pool" into ["pnpm", "dlx", "agent_pool"]
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let program = PathBuf::from(parts[0]);
        let prefix_args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        return (program, prefix_args);
    }

    if let Ok(path) = std::env::var("AGENT_POOL") {
        return (PathBuf::from(path), vec![]);
    }

    (PathBuf::from("agent_pool"), vec![])
}
```

Then:

```rust
let (program, prefix_args) = resolve_agent_pool_command();
let output = Command::new(&program)
    .args(&prefix_args)
    .arg("submit_task")
    .args(&cli_args)
    .output()?;
```

**Pros:**
- No shell escaping issues
- Cross-platform (no sh/cmd difference)
- Faster (no shell overhead)

**Cons:**
- Doesn't handle quoted arguments in AGENT_POOL_COMMAND
- Less flexible than shell invocation

### Option C: Auto-detect node_modules/.bin

In addition to the above, auto-detect the binary in common locations:

```rust
fn resolve_agent_pool_binary() -> PathBuf {
    // 1. Explicit command
    if let Ok(cmd) = std::env::var("AGENT_POOL_COMMAND") { ... }

    // 2. Explicit path
    if let Ok(path) = std::env::var("AGENT_POOL") { ... }

    // 3. Check node_modules/.bin/agent_pool
    let local_bin = Path::new("./node_modules/.bin/agent_pool");
    if local_bin.exists() {
        return local_bin.canonicalize().unwrap_or_else(|_| local_bin.to_path_buf());
    }

    // 4. Default: PATH
    PathBuf::from("agent_pool")
}
```

**Pros:**
- Zero config for npm/pnpm users who install locally
- Just works after `pnpm add agent_pool`

**Cons:**
- Implicit behavior might surprise users
- Only works when running from project root

## Recommendation

**Implement Option B + Option C together:**

1. Add `AGENT_POOL_COMMAND` with simple whitespace splitting (Option B)
2. Auto-detect `./node_modules/.bin/agent_pool` (Option C)

Resolution order:
1. `AGENT_POOL_COMMAND` - explicit command (e.g., `pnpm dlx agent_pool`)
2. `AGENT_POOL` - explicit binary path
3. `./node_modules/.bin/agent_pool` - local npm install
4. `agent_pool` in PATH

This covers:
- Package manager users who want explicit control
- npm/pnpm users who just `pnpm add agent_pool`
- Users with global installs

## Open Questions

1. **Windows support** - Should `AGENT_POOL_COMMAND` work on Windows? Would need different shell handling or restrict to Option B only.

2. **Relative vs absolute paths** - Should `./node_modules/.bin/agent_pool` be canonicalized? Subprocess might run from different directory.

3. **Error messages** - When binary not found, should we suggest checking `AGENT_POOL_COMMAND` or installing via npm?

## Testing

- Test with `AGENT_POOL_COMMAND="echo"` to verify args are passed correctly
- Test with local `node_modules/.bin/agent_pool` detection
- Test on macOS, Linux, Windows

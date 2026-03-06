# Inlined Config

**Status:** Not started

**Depends on:** Nothing

**Blocks:** STATE_PERSISTENCE (config must be fully resolved before serializing)

## Motivation

Currently, config parsing may defer file reads (e.g., schema references, script paths). This creates issues:
1. FS reads during execution can fail unexpectedly
2. Can't serialize config to state file without resolving references first
3. Harder to reason about what the config "is" at any point

## Goal

After `Config::parse()`, all file references are resolved. The resulting `Config` struct is fully self-contained - no further FS reads needed.

## Current State

File reads happen at different times:

1. **`SchemaRef::Link`** (value_schema) - Read in `CompiledSchemas::compile()` which is called at startup. **OK**

2. **`Instructions::Link`** - Read in `generate_step_docs()` which is called **per task execution**. **PROBLEM**

3. **`Action::Command { script }`** - Just a string, not a file path. **OK**

The main issue is `Instructions::Link` - linked markdown files are read every time we build the agent payload, not at startup.

## Proposed Changes

1. **All file reads happen during parsing** - If a config references a file, read it during `Config::parse()` or `Config::load()`

2. **Store resolved content, not paths** - Where we currently store a path to a schema file, store the parsed schema instead

3. **Clear separation** - `ConfigFile` (raw JSON structure) vs `Config` (fully resolved, validated, ready to run)

## Implementation

### Phase 1: Inline Instructions at Config Load

**Changes:**
- Add `InlinedConfig` struct that mirrors `Config` but with all links resolved
- `Instructions::Link { link }` becomes `Instructions::Inline(String)` with file content
- Create `Config::inline(base_path: &Path) -> io::Result<InlinedConfig>`
- Call this once at startup, store `InlinedConfig`

**Result:** `generate_step_docs()` never reads files - instructions are already inline.

### Phase 2: Store InlinedConfig in Runner

**Changes:**
- `TaskRunner` holds `InlinedConfig` instead of `&Config`
- Remove `config_base_path` from `TaskRunner` (no longer needed for file resolution)

### Serialization for STATE_PERSISTENCE

The `InlinedConfig` is what gets serialized to `config.json` in the run folder. All references resolved, ready to resume without the original files.

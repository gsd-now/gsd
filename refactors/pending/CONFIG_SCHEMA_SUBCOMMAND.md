# Config and Task Schema Generation

**Status:** Not started

## Motivation

Users need a way to validate their config files and get IDE autocomplete. JSON schema generation enables:

1. **Validation**: `gsd config validate config.jsonc`
2. **IDE integration**: Point VSCode/IntelliJ at the schema for autocomplete
3. **Documentation**: Schema serves as authoritative reference for config format

## Current State

- Config is defined in `crates/gsd_config/src/config.rs` using serde
- Task payload is defined somewhere (need to verify location)
- No way to extract the schema programmatically
- Users must read code or examples to understand config format

## Proposed Changes

### Command Structure

Two subcommand groups:

```bash
# Task schema (what agents receive)
gsd schema docs       # Print task JSON schema (pretty)
gsd schema validate   # Validate a task file against schema

# Config schema (gsd.config.json)
gsd config schema     # Print config JSON schema (pretty)
gsd config docs       # Print config schema with documentation
gsd config validate   # Validate a config file against schema
```

### 1. Add schemars dependency

Add `schemars` crate to derive JSON Schema from Rust types (same as isograph):

```toml
# crates/gsd_config/Cargo.toml
[dependencies]
schemars = "0.8"
```

### 2. Add $schema field to config struct

Allow users to specify the schema in their config file (like isograph):

```rust
// crates/gsd_config/src/config.rs
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The user may hard-code the JSON Schema for their version of the config.
    #[serde(rename = "$schema")]
    pub json_schema: Option<String>,

    // ... existing fields ...
}
```

This allows config files to include:
```json
{
  "$schema": "https://unpkg.com/@gsd/cli@latest/gsd-config-schema.json",
  "tasks": [...]
}
```

### 3. Derive JsonSchema on config types

```rust
// crates/gsd_config/src/config.rs
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "$schema")]
    pub json_schema: Option<String>,
    // ...
}
```

### 4. Create build-time schema generator binary

Following isograph's pattern, create a separate binary that generates the schema:

```rust
// crates/gsd_config/src/bin/build_json_schema.rs
use gsd_config::Config;
use schemars::schema_for;
use std::fs;

fn main() {
    let schema = schema_for!(Config);

    fs::write(
        "./libs/gsd-cli/gsd-config-schema.json",
        serde_json::to_string_pretty(&schema).unwrap(),
    )
    .unwrap();
}
```

### 5. Schema file location

Generate schema to `libs/gsd-cli/gsd-config-schema.json` (or similar npm package directory) so it gets published with the npm package and can be referenced via unpkg/jsdelivr:

```
https://unpkg.com/@gsd/cli@latest/gsd-config-schema.json
```

### 6. Add subcommands to CLI

```rust
// crates/gsd_cli/src/main.rs

#[derive(Subcommand)]
enum SchemaCommands {
    /// Print task JSON schema
    Docs,
    /// Validate a task file against schema
    Validate { file: PathBuf },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print config JSON schema
    Schema,
    /// Print config schema with documentation
    Docs,
    /// Validate a config file against schema
    Validate { file: PathBuf },
}
```

### 7. Add CI check

Add a CI step that runs the schema generator and verifies the output matches what's committed (or just generates and doesn't commit):

```yaml
# .github/workflows/ci.yml
- name: Generate JSON schema
  run: cargo run -p gsd_config --bin build_json_schema
```

Since we generate at build time and publish to npm, we don't need to check in the generated file. CI just verifies the generator runs without error.

## Files to Change

- `crates/gsd_config/Cargo.toml` - add schemars, add bin target
- `crates/gsd_config/src/config.rs` - derive JsonSchema, add $schema field
- `crates/gsd_config/src/bin/build_json_schema.rs` - new binary
- `crates/gsd_cli/src/main.rs` - add schema/config subcommands
- `libs/gsd-cli/` - schema output location (npm package)
- `.github/workflows/ci.yml` - add schema generation check

## Design Decisions

1. **Use schemars (same as isograph)** - proven approach, works well
2. **$schema field with serde rename** - allows users to get IDE autocomplete by adding the field
3. **Generate at build time** - schema is static, no need for runtime generation
4. **Output to libs/ for npm** - makes schema accessible via CDN for IDE integration
5. **Both task and config schemas** - users need both for full IDE support

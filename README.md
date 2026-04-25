<div align="center">
  <img src="https://raw.githubusercontent.com/aiperceivable/apcore-cli/main/apcore-cli-logo.svg" alt="apcore-cli logo" width="200"/>
</div>

# apcore-cli (Rust)

Terminal adapter for apcore. Execute AI-Perceivable modules from the command line.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-passing-brightgreen.svg)]()

| | |
|---|---|
| **Rust SDK** | [github.com/aiperceivable/apcore-cli-rust](https://github.com/aiperceivable/apcore-cli-rust) |
| **Python SDK** | [github.com/aiperceivable/apcore-cli-python](https://github.com/aiperceivable/apcore-cli-python) |
| **Spec repo** | [github.com/aiperceivable/apcore-cli](https://github.com/aiperceivable/apcore-cli) |
| **apcore core** | [github.com/aiperceivable/apcore](https://github.com/aiperceivable/apcore) |

**apcore-cli** turns any [apcore](https://github.com/aiperceivable/apcore)-based project into a fully featured CLI tool -- with **zero code changes** to your existing modules.

```
┌──────────────────┐
│  your-apcore     │  <- your existing apcore project (unchanged)
│  ...             │
└────────┬─────────┘
         │  extensions directory
         v
┌──────────────────┐
│   apcore-cli     │  <- just install & point to extensions dir
└───┬──────────┬───┘
    │          │
    v          v
 Terminal    Unix
 Commands    Pipes
```

## Design Philosophy

- **Zero intrusion** -- your apcore project needs no code changes, no imports, no dependencies on apcore-cli
- **Zero configuration** -- point to an extensions directory, everything is auto-discovered
- **Pure adapter** -- apcore-cli reads from the apcore Registry; it never modifies your modules
- **Unix-native** -- JSON output for pipes, rich tables for terminals, STDIN input, shell completions

## Installation

```bash
cargo install apcore-cli
```

Requires Rust 1.75+ and `apcore = 0.17.1` (exact pin).

## Quick Start

### Try it now

The repo includes 8 example modules you can run immediately:

```bash
git clone https://github.com/aiperceivable/apcore-cli-rust.git
cd apcore-cli-rust
make build                       # compile release binary to .bin/

# Add .bin to PATH for this session
export PATH=.bin:$PATH

# Run a module
apcore-cli --extensions-dir examples/extensions math.add --a 5 --b 10
# {"sum": 15}

# Or use the exec subcommand
apcore-cli --extensions-dir examples/extensions exec math.add --a 5 --b 10

# List all modules
apcore-cli --extensions-dir examples/extensions list --format json

# Run all examples
bash examples/run_examples.sh
```

> **Note:** If you have the Python version of `apcore-cli` installed, `make build` places
> the Rust binary at `.bin/apcore-cli`. Prepend `.bin` to your PATH (as shown above) to
> use the Rust version in this project.

See [Examples](#examples) for the full list of example modules and usage patterns.

### Zero-code approach

If you already have an apcore-based project with an extensions directory:

```bash
# Execute a module
apcore-cli --extensions-dir ./extensions math.add --a 42 --b 58

# Or set the env var once
export APCORE_EXTENSIONS_ROOT=./extensions
apcore-cli math.add --a 42 --b 58
```

All modules are auto-discovered. CLI flags are auto-generated from each module's JSON Schema.

### Programmatic approach (Rust library)

Use `apcore-cli` as a library crate to embed CLI into your own binary:

```rust
use apcore_cli::CliConfig;
use std::sync::Arc;

// Pre-populated registry from your framework (skips filesystem discovery)
let config = CliConfig {
    prog_name: Some("myapp".to_string()),
    registry: Some(Arc::new(my_provider)),
    executor: Some(Arc::new(my_executor)),
    ..Default::default()
};
// Use config.registry / config.executor at dispatch time
```

### Exposure Filtering (FE-12)

`apcore-cli` supports declarative module exposure filtering via `ExposureFilter`.
Because Rust's `CliConfig` does not currently have an `expose` field, filtering is
applied via a builder method on `GroupedModuleGroup` -- construct the group first,
then attach the filter:

```rust
use apcore_cli::{ExposureFilter, GroupedModuleGroup};
use std::sync::Arc;

// Option 1: construct ExposureFilter directly (mode, include patterns, exclude patterns)
let filter = ExposureFilter::new(
    "include",
    &["admin.*".to_string()],
    &[],
);

// Option 2: load from a JSON config value
let cfg = serde_json::json!({
    "mode": "exclude",
    "exclude": ["debug.*", "test.*"]
});
let filter = ExposureFilter::from_config(&cfg).expect("valid exposure config");

// Wire the filter into the grouped module group
let group = GroupedModuleGroup::new(registry.clone(), executor.clone(), 1000)
    .with_exposure_filter(filter);

// Then continue building your clap::Command from the group...
```

> **Note:** Exposure filtering must be wired via `GroupedModuleGroup::with_exposure_filter`
> -- the `CliConfig` struct does not currently expose this field. See CHANGELOG 0.6.0 /
> FE-12 for background.

## Integration with Existing Projects

### Typical apcore project structure

```
your-project/
├── extensions/          <- modules live here
│   ├── math/
│   │   └── add.rs
│   ├── text/
│   │   └── upper.rs
│   └── ...
├── your_app.rs          <- your existing code (untouched)
└── ...
```

### Adding CLI support

No changes to your project. Just install and run:

```bash
cargo install apcore-cli
apcore-cli --extensions-dir ./extensions list
apcore-cli --extensions-dir ./extensions math.add --a 5 --b 10
```

### STDIN piping (Unix pipes)

```bash
# Pipe JSON input
echo '{"a": 100, "b": 200}' | apcore-cli math.add --input -
# {"sum": 300}

# CLI flags override STDIN values
echo '{"a": 1, "b": 2}' | apcore-cli math.add --input - --a 999
# {"sum": 1001}

# Chain with other tools
apcore-cli sysutil.info | jq '.os, .hostname'
```

## CLI Reference

```
apcore-cli [OPTIONS] COMMAND [ARGS]
```

### Global Options

| Option | Default | Description |
|--------|---------|-------------|
| `--extensions-dir` | `./extensions` | Path to apcore extensions directory |
| `--log-level` | `WARNING` | Logging: `DEBUG`, `INFO`, `WARNING`, `ERROR` |
| `--version` | | Show version and exit |
| `--help` | | Show help and exit |
| `--verbose` | | Show all options in help (including built-in apcore options) |
| `--man` | | Output man page in roff format (use with `--help`) |

### Built-in Commands

apcore-cli ships with 13 built-in subcommands, all reachable under the reserved `apcli` group (canonical list: `APCLI_SUBCOMMAND_NAMES` in `src/builtin_group.rs`). The reserved top-level group name is `RESERVED_GROUP_NAMES = ["apcli"]`. They fall into four groups:

> **v0.7 note:** The previous 14-entry `BUILTIN_COMMANDS` constant in `src/cli.rs` was retired and is now `#[deprecated]`. Use `APCLI_SUBCOMMAND_NAMES` for the canonical list and `RESERVED_GROUP_NAMES` for collision detection.

**Module invocation**

| Command | Description | Source |
|---------|-------------|--------|
| `exec <module_id>` | Execute a module by ID (supports `--input`, `--yes`, `--format`, `--sandbox`, `--dry-run`, `--trace`, `--stream`, `--strategy`, `--fields`, `--approval-timeout`, `--approval-token`) | `cli` |
| `list` | List available modules with filtering (`--tag`, `--search`, `--status`, `--annotation`, `--sort`, `--reverse`, `--deprecated`, `--deps`) | `discovery` |
| `describe <module_id>` | Show full module metadata, schemas, and annotations | `discovery` |
| `validate <module_id>` | Run preflight schema / approval / dependency validation without executing | `validate` |

**System management**

| Command | Description | Source |
|---------|-------------|--------|
| `health` | Report framework / registry / executor health | `system_cmd` |
| `usage` | Show cumulative execution statistics | `system_cmd` |
| `enable <module_id>` | Enable a previously disabled module | `system_cmd` |
| `disable <module_id>` | Disable a module (persists until re-enabled) | `system_cmd` |
| `reload` | Reload registry from the extensions directory | `system_cmd` |
| `config` | Show resolved configuration (Config Bus namespaces included) | `system_cmd` |

**Workflow**

| Command | Description | Source |
|---------|-------------|--------|
| `init` | Scaffold a new extensions directory with example modules | `init_cmd` |
| `describe-pipeline <pipeline_id>` | Show pipeline execution strategy and stage trace | `strategy` |

**Shell integration**

| Command | Description | Source |
|---------|-------------|--------|
| `completion <shell>` | Generate shell completion script (bash/zsh/fish/elvish/powershell) | `shell` |
| `man <command>` | Generate man page in roff format (or `--help --man` for the full program page) | `shell` |

### Module Execution Options

When executing a module (e.g. `apcore-cli math.add` or `apcore-cli exec math.add`), these built-in options are available (hidden by default; use `--verbose` to show in `--help`):

| Option | Description |
|--------|-------------|
| `--input -` | Read JSON input from STDIN |
| `--yes` / `-y` | Bypass approval prompts |
| `--large-input` | Allow STDIN input larger than 10MB |
| `--format <fmt>` | Output format: `json`, `table`, `csv`, `yaml`, or `jsonl` |
| `--sandbox` | Run module in subprocess sandbox |
| `--dry-run` | Run preflight checks without executing (FE-11, routed through the `validate` module) |
| `--trace` | Emit a pipeline execution trace |
| `--stream` | Stream results line-by-line instead of buffering |
| `--strategy <name>` | Override execution strategy (`standard`, `internal`, `testing`, `performance`, `minimal`) |
| `--fields <csv>` | Select output fields via dot-path notation |
| `--approval-timeout <seconds>` | Override approval prompt timeout (default: 60) |
| `--approval-token <token>` | Provide a pre-obtained approval token to skip the interactive prompt |

Schema-generated flags (e.g. `--a`, `--b`) are added automatically from the module's `input_schema`.

**Enhanced `list` flags (v0.6.0):** `--search <query>`, `--status <active|disabled|deprecated>`, `--annotation <key=value>`, `--sort <field>`, `--reverse`, `--deprecated` (include deprecated modules), `--deps` (show dependency graph).

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Module execution error |
| `2` | Invalid CLI input |
| `44` | Module not found / disabled / load error |
| `45` | Schema validation error |
| `46` | Approval denied or timed out |
| `47` | Configuration error |
| `48` | Schema circular reference |
| `65` | `EXIT_CONFIG_BIND_ERROR` -- Configuration bind to struct failed (Config Bus) |
| `66` | `EXIT_CONFIG_MOUNT_ERROR` -- Configuration namespace mount failed (Config Bus) |
| `70` | `EXIT_ERROR_FORMATTER_DUPLICATE` -- Duplicate error formatter registration |
| `77` | ACL denied |
| `78` | `EXIT_CONFIG_NAMESPACE_*` -- Namespace reserved / duplicate / env-prefix conflict / env-map conflict (Config Bus) |
| `130` | Execution cancelled (Ctrl+C) |

## Configuration

apcore-cli uses a 4-tier configuration precedence:

1. **CLI flag** (highest): `--extensions-dir ./custom`
2. **Environment variable**: `APCORE_EXTENSIONS_ROOT=./custom`
3. **Config file**: `apcore.yaml`
4. **Default** (lowest): `./extensions`

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `APCORE_EXTENSIONS_ROOT` | Path to extensions directory | `./extensions` |
| `APCORE_CLI_AUTO_APPROVE` | Set to `1` to bypass all approval prompts | *(unset)* |
| `APCORE_CLI_LOGGING_LEVEL` | CLI-specific log level (takes priority over `APCORE_LOGGING_LEVEL`) | `WARNING` |
| `APCORE_LOGGING_LEVEL` | Global apcore log level (fallback when `APCORE_CLI_LOGGING_LEVEL` is unset) | `WARNING` |
| `APCORE_AUTH_API_KEY` | API key for remote registry authentication | *(unset)* |
| `APCORE_CLI_SANDBOX` | Set to `1` to enable subprocess sandboxing | *(unset)* |
| `APCORE_CLI_HELP_TEXT_MAX_LENGTH` | Maximum characters for CLI option help text before truncation | `1000` |
| `APCORE_CLI_APPROVAL_TIMEOUT` | Default approval prompt timeout in seconds (overridable via `--approval-timeout`) | `60` |
| `APCORE_CLI_STRATEGY` | Default execution strategy (overridable via `--strategy`) | `standard` |
| `APCORE_CLI_GROUP_DEPTH` | Maximum module-grouping depth when building the clap command tree | `1000` |

### Config File (`apcore.yaml`)

```yaml
extensions:
  root: ./extensions
logging:
  level: DEBUG
sandbox:
  enabled: false
cli:
  help_text_max_length: 1000
  approval_timeout: 60          # seconds
  strategy: standard            # standard | internal | testing | performance | minimal
  group_depth: 1000             # max module-grouping depth
```

## Features

- **Auto-discovery** -- all modules in the extensions directory are found and exposed as CLI commands
- **Auto-generated flags** -- JSON Schema `input_schema` is converted to `--flag value` CLI options with type validation
- **Boolean flag pairs** -- `--verbose` / `--no-verbose` from `"type": "boolean"` schema properties
- **Enum choices** -- `"enum": ["json", "csv"]` becomes `--format json` with clap validation
- **STDIN piping** -- `--input -` reads JSON from STDIN, CLI flags override for duplicate keys
- **TTY-adaptive output** -- comfy-table for terminals, JSON for pipes (configurable via `--format`)
- **Approval gate** -- TTY-aware HITL prompts for modules with `requires_approval: true`, with `--yes` bypass and 60s timeout
- **Schema validation** -- inputs validated against JSON Schema before execution, with `$ref`/`allOf`/`anyOf`/`oneOf` resolution
- **Security** -- API key auth (keyring + AES-256-GCM), append-only audit logging, subprocess sandboxing
- **Shell completions** -- `apcore-cli completion bash|zsh|fish|elvish|powershell` generates completion scripts
- **Man pages** -- `apcore-cli man <command>` for single commands, or `--help --man` for a complete program man page. `build_program_man_page()` provides one-line integration for downstream projects
- **Documentation URL** -- `set_docs_url()` adds doc links to help footers and man pages
- **Audit logging** -- all executions logged to `~/.apcore-cli/audit.jsonl` with SHA-256 input hashing

## How It Works

### Mapping: apcore to CLI

| apcore | CLI |
|--------|-----|
| `module_id` (`math.add`) | Command name (`apcore-cli math.add` or `apcore-cli exec math.add`) |
| `description` | `--help` text |
| `input_schema.properties` | CLI flags (`--a`, `--b`) |
| `input_schema.required` | Validated post-collection (required fields shown as `[required]` in `--help`) |
| `annotations.requires_approval` | HITL approval prompt |

### Architecture

```
User / AI Agent (terminal)
    |
    v
apcore-cli (the adapter)
    |
    +-- ConfigResolver       4-tier config precedence
    +-- LazyModuleGroup      Dynamic clap command generation
    +-- SchemaParser         JSON Schema -> clap options
    +-- RefResolver          $ref / allOf / anyOf / oneOf
    +-- ApprovalGate         TTY-aware HITL approval (tokio::select!)
    +-- OutputFormatter      TTY-adaptive JSON/table output (comfy-table)
    +-- AuditLogger          JSON Lines execution logging
    +-- Sandbox              tokio subprocess isolation
    |
    v
apcore Registry + Executor (your modules, unchanged)
```

## API Overview

The following items are re-exported at the crate root (`apcore_cli::*`). Everything else lives under its module path (e.g. `apcore_cli::cli::LazyModuleGroup`).

### Structs

`CliConfig`, `GroupedModuleGroup`, `ExposureFilter`, `ConfigResolver`, `AuditLogger`, `AuthProvider`, `ConfigEncryptor`, `Sandbox`, `CliApprovalHandler`, `FsDiscoverer`, `ApCoreRegistryProvider`, `ListOptions`, `SchemaArgs`, `BoolFlagPair`.

> Note: `LazyModuleGroup` is **not** re-exported at the crate root -- access it as `apcore_cli::cli::LazyModuleGroup`.

### Functions

Organized by source module:

- **`approval::`** `check_approval`, `check_approval_with_tty`
- **`cli::`** `set_verbose_help`, `is_verbose_help`, `set_docs_url`, `get_docs_url`, `set_executables`, `set_audit_logger`, `exec_command`, `build_module_command`, `build_module_command_with_limit`, `collect_input`, `collect_input_from_reader`, `validate_module_id`, `dispatch_module`
- **`discovery::`** `validate_tag`, `cmd_list`, `cmd_list_enhanced`, `cmd_describe`
  - Per-subcommand registrars (v0.7+, preferred): `register_list_command`, `register_describe_command`, `register_exec_command`, `register_validate_command`
  - Batch shim (deprecated, use per-subcommand registrars): `register_discovery_commands`
- **`display_helpers::`** `get_display`, `get_cli_display_fields`
- **`exposure::`** `glob_match`
- **`init_cmd::`** `init_command`, `handle_init`
- **`output::`** `resolve_format`, `format_module_list`, `format_module_list_with_deps`, `format_module_detail`, `format_exec_result`
- **`ref_resolver::`** `resolve_refs`
- **`schema_parser::`** `prop_name_to_flag_name`, `extract_help_with_limit`, `map_type`, `schema_to_clap_args`, `schema_to_clap_args_with_limit`, `reconvert_enum_values`
- **`shell::`** `register_completion_command`, `completion_command`, `cmd_completion`, `man_command`, `build_synopsis`, `generate_man_page`, `cmd_man`, `has_man_flag`, `build_program_man_page`, `generate_grouped_bash_completion`, `generate_grouped_zsh_completion`, `generate_grouped_fish_completion`
  - Batch shim (deprecated): `register_shell_commands`
- **`strategy::`** `describe_pipeline_command`, `register_pipeline_command`, `dispatch_describe_pipeline`
- **`system_cmd::`** `dispatch_health`, `dispatch_usage`, `dispatch_enable`, `dispatch_disable`, `dispatch_reload`, `dispatch_config`
  - Per-subcommand registrars (v0.7+, preferred): `register_health_command`, `register_usage_command`, `register_enable_command`, `register_disable_command`, `register_reload_command`, `register_config_command`
  - Batch shim (deprecated): `register_system_commands`
- **`validate::`** `validate_command`, `register_validate_command`, `dispatch_validate`, `format_preflight_result`

### Traits

`RegistryProvider`

### Errors

Each module defines its own `thiserror::Error` enum rather than a single catch-all type:

- `ApprovalError` -- `Denied` / `NonInteractive` / `Timeout`
- `CliError` -- `InvalidModuleId` / `ReservedModuleId` / `StdinRead` / `JsonParse` / `InputTooLarge` / `NotAnObject`
- `DiscoveryError` -- `ModuleNotFound` / `InvalidModuleId` / `InvalidTag`
- `SchemaParserError` -- `FlagCollision`
- `RefResolverError` -- `Unresolvable` / `Circular` / `MaxDepthExceeded`
- `ShellError` -- `UnknownCommand`
- `AuthenticationError` -- `MissingApiKey` / `InvalidApiKey` / `KeyringError` / `RequestError`
- `ConfigDecryptionError` -- `AuthTagMismatch` / `InvalidUtf8` / `KeyringError` / `KdfError`
- `ModuleExecutionError` -- `NonZeroExit` / `Timeout` / `OutputParseFailed` / `SpawnFailed`

## Development

### Getting Started

The conformance suite under `tests/conformance_apcli_visibility.rs` reads
shared fixtures from the **spec repo** (`aiperceivable/apcore-cli`). Clone
it as a sibling of this repo, or point `APCORE_CLI_SPEC_REPO` at an
existing checkout:

```bash
# One-time: clone both repos side by side
git clone https://github.com/aiperceivable/apcore-cli.git
git clone https://github.com/aiperceivable/apcore-cli-rust.git

cd apcore-cli-rust
make setup                       # install apdev-rs + git pre-commit hook
make build                       # compile release binary to .bin/
export PATH=.bin:$PATH           # use Rust version in this session
```

Alternative layout (spec repo checked out elsewhere):

```bash
export APCORE_CLI_SPEC_REPO=/path/to/apcore-cli
cargo test --all-features
```

CI clones the spec repo automatically — see `.github/workflows/ci.yml`.

### Daily Workflow

```bash
# Build and run
make build                       # release build + symlink to .bin/apcore-cli
apcore-cli --extensions-dir examples/extensions list

# Run all checks (same as pre-commit hook: fmt + clippy + tests)
make check

# Run individual steps
cargo fmt --all -- --check       # formatting check
cargo clippy --all-targets --all-features -- -D warnings   # lint
cargo test --all-features        # run full test suite
```

### Adding a New Module Descriptor

Each module is discovered via a `module.json` file in the extensions directory:

```
extensions/
└── math/
    └── add/
        └── module.json          <- descriptor file
```

```json
{
  "name": "math.add",
  "description": "Add two integers and return their sum",
  "tags": ["math"],
  "input_schema": {
    "type": "object",
    "properties": {
      "a": { "type": "integer", "description": "First operand" },
      "b": { "type": "integer", "description": "Second operand" }
    },
    "required": ["a", "b"]
  },
  "output_schema": {
    "type": "object",
    "properties": {
      "sum": { "type": "integer" }
    }
  }
}
```

The CLI auto-discovers all `module.json` files recursively under `--extensions-dir`.


### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap 4` | CLI framework (derive + env + string) |
| `tokio 1` | Async runtime (process, signal, time) |
| `serde` / `serde_json` / `serde_yaml` | Serialization |
| `comfy-table 6` | Terminal table rendering |
| `aes-gcm` / `sha2` / `pbkdf2` | Cryptography |
| `keyring 2` | OS keyring access |
| `clap_complete 4` | Shell completion generation |
| `thiserror` / `anyhow` | Error handling |
| `tracing` | Structured logging |

## Examples

The repo includes 8 runnable example modules and a guide for writing your own.

```bash
make build && export PATH=.bin:$PATH
export APCORE_EXTENSIONS_ROOT=examples/extensions

apcore-cli math.add --a 5 --b 10          # {"sum": 15}
apcore-cli list --tag math                # filter by tag
apcore-cli describe math.add --format json # full schema
bash examples/run_examples.sh             # run all 8 modules
```

See [examples/README.md](examples/README.md) for the full module list, authoring guide, and STDIN piping patterns.

## License

Apache-2.0

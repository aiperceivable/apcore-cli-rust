<div align="center">
  <img src="https://raw.githubusercontent.com/aipartnerup/apcore-cli/main/apcore-cli-logo.svg" alt="apcore-cli logo" width="200"/>
</div>

# apcore-cli (Rust)

Terminal adapter for apcore. Execute AI-Perceivable modules from the command line.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-452%20passed-brightgreen.svg)]()

| | |
|---|---|
| **Rust SDK** | [github.com/aipartnerup/apcore-cli-rust](https://github.com/aipartnerup/apcore-cli-rust) |
| **Python SDK** | [github.com/aipartnerup/apcore-cli-python](https://github.com/aipartnerup/apcore-cli-python) |
| **Spec repo** | [github.com/aipartnerup/apcore-cli](https://github.com/aipartnerup/apcore-cli) |
| **apcore core** | [github.com/aipartnerup/apcore](https://github.com/aipartnerup/apcore) |

**apcore-cli** turns any [apcore](https://github.com/aipartnerup/apcore)-based project into a fully featured CLI tool -- with **zero code changes** to your existing modules.

```
┌──────────────────┐
│  django-apcore   │  <- your existing apcore project (unchanged)
│  flask-apcore    │
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

Requires Rust 1.75+ and `apcore >= 0.13.0`.

## Quick Start

### Try it now

The repo includes 8 example modules you can run immediately:

```bash
git clone https://github.com/aipartnerup/apcore-cli-rust.git
cd apcore-cli-rust
cargo build --release

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

### Built-in Commands

| Command | Description |
|---------|-------------|
| `exec <module_id>` | Execute a module by ID (with `--input`, `--yes`, `--format`, `--sandbox` flags) |
| `list` | List available modules with optional tag filtering |
| `describe <module_id>` | Show full module metadata and schemas |
| `completion <shell>` | Generate shell completion script (bash/zsh/fish/elvish/powershell) |
| `man <command>` | Generate man page in roff format |

### Module Execution Options

When executing a module (e.g. `apcore-cli math.add` or `apcore-cli exec math.add`), these built-in options are always available:

| Option | Description |
|--------|-------------|
| `--input -` | Read JSON input from STDIN |
| `--yes` / `-y` | Bypass approval prompts |
| `--large-input` | Allow STDIN input larger than 10MB |
| `--format` | Output format: `json` or `table` |
| `--sandbox` | Run module in subprocess sandbox |

Schema-generated flags (e.g. `--a`, `--b`) are added automatically from the module's `input_schema`.

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
| `77` | ACL denied |
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

### Config File (`apcore.yaml`)

```yaml
extensions:
  root: ./extensions
logging:
  level: DEBUG
sandbox:
  enabled: false
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
- **Man pages** -- `apcore-cli man <command>` generates roff-formatted man pages
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

## Examples

The `examples/extensions/` directory contains 8 runnable modules:

| Module | Description | Usage |
|--------|-------------|-------|
| `math.add` | Add two integers | `apcore-cli math.add --a 5 --b 10` |
| `math.multiply` | Multiply two integers | `apcore-cli math.multiply --a 6 --b 7` |
| `text.upper` | Uppercase a string | `apcore-cli text.upper --text hello` |
| `text.reverse` | Reverse a string | `apcore-cli text.reverse --text abcdef` |
| `text.wordcount` | Count words/chars/lines | `apcore-cli text.wordcount --text "hello world"` |
| `sysutil.info` | OS, hostname, Rust version | `apcore-cli sysutil.info` |
| `sysutil.env` | Read environment variables | `apcore-cli sysutil.env --name HOME` |
| `sysutil.disk` | Disk usage statistics | `apcore-cli sysutil.disk --path /` |

### Running examples

```bash
# Set extensions path (one time)
export APCORE_EXTENSIONS_ROOT=examples/extensions

# Execute modules
apcore-cli math.add --a 42 --b 58
apcore-cli text.upper --text "hello apcore"
apcore-cli sysutil.info
apcore-cli sysutil.disk --path /

# Discovery
apcore-cli list --format json
apcore-cli list --tag math --format json
apcore-cli describe math.add --format json

# STDIN piping
echo '{"a": 100, "b": 200}' | apcore-cli math.add --input -

# Shell completion
apcore-cli completion bash >> ~/.bashrc
apcore-cli completion zsh >> ~/.zshrc
apcore-cli completion fish > ~/.config/fish/completions/apcore-cli.fish

# Man pages
apcore-cli man list | man -l -

# Run all examples at once
bash examples/run_examples.sh
```

## Development

```bash
git clone https://github.com/aipartnerup/apcore-cli-rust.git
cd apcore-cli-rust
make setup                       # install dev tools + git hook
cargo build                      # build
make check                       # fmt + clippy + tests (same as pre-commit)
cargo test --all-features        # 452 tests
```

### Project Structure

```
src/
├── lib.rs                   # Library root, public API re-exports
├── main.rs                  # Binary entry point, clap wiring, dispatch
├── cli.rs                   # LazyModuleGroup, build_module_command, collect_input, dispatch_module
├── config.rs                # ConfigResolver (4-tier precedence)
├── schema_parser.rs         # JSON Schema -> clap options
├── ref_resolver.rs          # $ref / allOf / anyOf / oneOf resolution
├── output.rs                # TTY-adaptive output formatting (comfy-table)
├── discovery.rs             # list / describe commands, RegistryProvider trait
├── approval.rs              # HITL approval gate with tokio timeout
├── shell.rs                 # bash/zsh/fish/elvish/powershell completion + man pages
├── _sandbox_runner.rs       # Subprocess entry point for sandboxed execution
└── security/
    ├── mod.rs                # Exports
    ├── auth.rs               # API key authentication (Bearer header)
    ├── config_encryptor.rs   # Keyring + AES-256-GCM encrypted config
    ├── audit.rs              # JSON Lines audit logging (SHA-256 hashed inputs)
    └── sandbox.rs            # tokio subprocess-based execution isolation

examples/
├── run_examples.sh          # Run all examples end-to-end
└── extensions/
    ├── math/                # math.add, math.multiply
    ├── text/                # text.upper, text.reverse, text.wordcount
    └── sysutil/             # sysutil.info, sysutil.env, sysutil.disk

tests/
├── test_cli.rs              # CLI dispatcher tests
├── test_config.rs           # ConfigResolver tests
├── test_schema_parser.rs    # Schema-to-clap tests
├── test_ref_resolver.rs     # $ref resolution tests
├── test_output.rs           # Output formatting tests
├── test_discovery.rs        # Discovery command tests
├── test_approval.rs         # Approval gate unit tests
├── approval_integration.rs  # Approval gate integration tests
├── test_shell.rs            # Shell completion + man page tests
├── test_e2e.rs              # End-to-end binary tests
├── test_integration.rs      # Cross-component integration tests
└── security/                # Auth, audit, encryptor, sandbox tests
```

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

## License

Apache-2.0

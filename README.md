<div align="center">
  <img src="https://raw.githubusercontent.com/aiperceivable/apcore-cli/main/apcore-cli-logo.svg" alt="apcore-cli logo" width="200"/>
</div>

# apcore-cli (Rust)

Terminal adapter for apcore. Execute AI-Perceivable modules from the command line.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-459%20passed-brightgreen.svg)]()

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

Requires Rust 1.75+ and `apcore >= 0.13.0`.

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
| `APCORE_CLI_HELP_TEXT_MAX_LENGTH` | Maximum characters for CLI option help text before truncation | `1000` |

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

## Development

### Getting Started

```bash
git clone https://github.com/aiperceivable/apcore-cli-rust.git
cd apcore-cli-rust
make setup                       # install apdev-rs + git pre-commit hook
make build                       # compile release binary to .bin/
export PATH=.bin:$PATH           # use Rust version in this session
```

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
cargo test --all-features        # 459 tests
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

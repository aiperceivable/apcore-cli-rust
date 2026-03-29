# Changelog

All notable changes to this project will be documented in this file.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).


## [0.4.0] - 2026-03-28

### Added
- **Verbose help mode** — Built-in apcore options (`--input`, `--yes`, `--large-input`, `--format`, `--sandbox`) are now hidden from `--help` output by default. Pass `--help --verbose` to display the full option list including built-in options.
- **Universal man page generation** — `build_program_man_page()` generates a complete roff man page covering all registered commands. `--help --man` outputs the man page, enabling downstream projects to get man pages for free.
- **Documentation URL support** — `set_docs_url()` sets a base URL for online docs. Per-command help shows `Docs: {url}/commands/{name}`, man page SEE ALSO includes `Full documentation at {url}`. No default — disabled when not set.

### Changed
- `build_module_command_with_limit()` and `add_dispatch_flags()` respect the global verbose help flag to control built-in option visibility.
- `--sandbox` is now always hidden from help (not yet implemented). Only four built-in options (`--input`, `--yes`, `--large-input`, `--format`) toggle with `--verbose`.
- Improved built-in option descriptions for clarity.

## [0.3.0] - 2026-03-27

### Added
- **Grouped CLI commands (FE-09)** — `GroupedModuleGroup` organizes modules into nested subcommand groups by namespace prefix, enabling `apcore-cli <group> <command>` invocation.
- **Display overlay helpers** — `get_display()` and `get_cli_display_fields()` resolve alias, description, and tags from `metadata["display"]`.
- **Init command (FE-10)** — `apcore-cli init module <id>` scaffolds new modules with `--style` (decorator/convention/binding), `--dir`, and `--description` options.
- **Grouped shell completions** — Bash, Zsh, and Fish completions now support two-level group/command completion via `_APCORE_GRP`.
- **Optional apcore-toolkit integration** — `DisplayResolver` and `RegistryWriter` via `toolkit` feature flag with graceful fallback.
- **Path traversal validation** — `--dir` rejects paths containing `..` components.

### Changed
- `BUILTIN_COMMANDS` updated to include `init` (6 items, sorted).
- `APCORE_AUTH_API_KEY` added to man page ENVIRONMENT section.
- Dependency bump: `apcore >= 0.14`.

## [0.2.2] - 2026-03-22

### Changed
- Rebrand: aipartnerup → aiperceivable

## [0.2.1] - 2026-03-19

### Changed
- Help text truncation limit increased from 200 to 1000 characters (`HELP_TEXT_MAX_LEN` constant)
- `cli.help_text_max_length` config key added to `ConfigResolver::DEFAULTS` (default: 1000)
- `logging.level` default changed from `"INFO"` to `"WARNING"` in `ConfigResolver::DEFAULTS` — aligns with Python/TypeScript SDKs and updated spec

### Added
- `extract_help_with_limit` — configurable-limit variant of `extract_help` (`schema_parser.rs`)
- `schema_to_clap_args_with_limit` — configurable-limit variant of `schema_to_clap_args` (`schema_parser.rs`)
- `build_module_command_with_limit` — accepts `help_text_max_length` parameter (`cli.rs`)
- `HELP_TEXT_MAX_LEN` constant exported from crate root (`lib.rs`)
- Test: `test_extract_help_truncates_at_1000`
- Test: `test_extract_help_no_truncation_within_limit`
- Test: `test_extract_help_custom_max_length`
- Test: `test_help_truncated_at_1000_chars` (integration)
- Test: `test_help_within_limit_not_truncated` (integration)
- 459 tests (up from 458)

## [0.2.0] - 2026-03-18

### Added

**Core Features (ported from apcore-cli-python 0.2.0)**

- **ConfigResolver** — 4-tier configuration precedence (CLI flag > env var > YAML file > defaults)
- **Core Dispatcher** — `validate_module_id`, `collect_input` (STDIN + CLI merge, 10MiB limit), `LazyModuleGroup` (lazy command cache), `build_module_command` (schema-to-clap), `dispatch_module` (full execution pipeline with SIGINT handling)
- **Schema Parser** — `schema_to_clap_args` converting JSON Schema to clap `Arg` instances, boolean flag pairs (`--flag`/`--no-flag`), enum choices with `PossibleValuesParser`, `reconvert_enum_values` for type coercion, `extract_help` with 200-char truncation
- **Ref Resolver** — `resolve_refs` with `$ref` inlining, `allOf` merge, `anyOf`/`oneOf` intersection, depth limit (32), circular detection
- **Output Formatter** — TTY-adaptive rendering (`comfy-table` for terminals, JSON for pipes), `format_module_list`, `format_module_detail`, `format_exec_result`, `resolve_format`, `truncate`
- **Discovery** — `list` command with AND tag filtering, `describe` command with exit-44 on not found, `RegistryProvider` trait, `ApCoreRegistryProvider` adapter
- **Approval Gate** — TTY-aware HITL prompts, `--yes` and `APCORE_CLI_AUTO_APPROVE=1` bypass, 60s `tokio::select!` timeout, `NonInteractive` error for non-TTY, all variants exit 46
- **Shell Integration** — `completion` command (bash/zsh/fish/elvish/powershell via `clap_complete`), `man` command (roff format with EXIT CODES and ENVIRONMENT sections)
- **Security** — `AuthProvider` (env/config/keyring with Bearer header), `ConfigEncryptor` (AES-256-GCM + PBKDF2, keyring fallback), `AuditLogger` (JSONL append, salted SHA-256 input hash), `Sandbox` (tokio subprocess, env whitelist, 300s timeout)

**Dispatch & Execution**

- `exec` subcommand — first-class clap subcommand for module execution
- External subcommand routing — `apcore-cli math.add --a 5` routes through `dispatch_module`
- Schema-derived flags — external subcommands look up module descriptor to build `--a`, `--b` etc. from `input_schema`
- `FsDiscoverer` — recursively scans extensions directory for `module.json` descriptors
- Script-based execution — modules with `run.sh` next to `module.json` execute as subprocesses (JSON stdin/stdout protocol)
- Path-traversal validation — executable paths canonicalized and verified to stay within extensions root

**Examples**

- 8 example modules: `math.add`, `math.multiply`, `text.upper`, `text.reverse`, `text.wordcount`, `sysutil.info`, `sysutil.env`, `sysutil.disk`
- Each module has `module.json` (descriptor) + `run.sh` (execution script)
- `examples/run_examples.sh` — runs all 15 demo scenarios
- `examples/README.md` — module authoring guide

**Developer Experience**

- `Makefile` with `setup`, `build`, `check` (fmt + clippy + tests), `clean` targets
- `.bin/` local binary directory to avoid PATH conflict with Python `apcore-cli`
- Pre-commit hook (fmt, clippy, check-chars)
- 458 tests across 17 test files, 0 failures
- `cargo clippy --all-targets --all-features -- -D warnings` clean

**Infrastructure**

- 10 exit codes matching the apcore protocol (0, 1, 2, 44, 45, 46, 47, 48, 77, 130)
- `add_dispatch_flags()` shared helper for exec and external subcommand flags
- `test-support` cargo feature for gating test utilities (`MockRegistry`, `mock_module`)
- Unified `RegistryProvider` trait (consolidated from separate `ModuleRegistry` + `RegistryProvider`)

### Dependencies

- `apcore` 0.13.0
- `clap` 4 (derive + env + string)
- `tokio` 1 (rt-multi-thread, macros, time, process, io-util, io-std, signal)
- `serde` + `serde_json` + `serde_yaml` 0.9
- `comfy-table` 6
- `aes-gcm` 0.10 + `sha2` 0.10 + `pbkdf2` 0.12
- `keyring` 2
- `clap_complete` 4
- `thiserror` 1 + `anyhow` 1
- `tracing` 0.1 + `tracing-subscriber` 0.3
- `reqwest` 0.12
- `async-trait` 0.1
- `base64` 0.22, `gethostname` 0.4, `chrono` 0.4, `dirs` 5, `tempfile` 3

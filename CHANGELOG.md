# Changelog

All notable changes to this project will be documented in this file.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).


## [0.8.0] - 2026-05-08

### Added

- **`CliError::SchemaParserFailure { module_id, source }` variant** in `src/cli.rs` — wraps `SchemaParserError::ReservedPropertyName` and `::FlagCollision` so both route to `EXIT_SCHEMA_CIRCULAR_REF` (48) via `CliError::exit_code()`. Previously these errors were re-wrapped as `CliError::InvalidModuleId` and exited with code 2, breaking cross-SDK exit-code parity with Python `sys.exit(48)` and TypeScript `process.exit(EXIT_CODES.SCHEMA_CIRCULAR_REF)`. Audit D11-NEW-005 (see Fixed).
- **Documented parity gap for the built-in-group rename feature** in `src/lib.rs`. Python `create_cli(builtin_group_name=...)` and TypeScript `createCli({ builtinGroupName })` ship the rename kwarg today; Rust does not because the embedding API (`CliConfig` / `run_with_config`) was removed in v0.7.0 (D9-001/002). The lib-level comment block now lists the implementation requirements (regex validation, `with_builtin_group_name` builder method, accessor on `CliConfig`, conversion of static `BUILTIN_GROUP_NAME` const to per-instance state) so the rename lands at the same time the embedding API is reintroduced.

### Fixed

- **D11-NEW-005 — `schema_to_clap_args` `Err(SchemaParserError::*)` was mapped to exit code 2**, not 48. The call site in `src/cli.rs:425` previously wrapped both `ReservedPropertyName` and `FlagCollision` as `CliError::InvalidModuleId`, which exits 2. Both are spec-defined exit-48 schema-validity errors per `apcore-cli/docs/features/schema-parser.md` Contract: `schema_to_click_options` Errors (cross-SDK parity with Python `sys.exit(48)` and TS `process.exit(EXIT_CODES.SCHEMA_CIRCULAR_REF)`). Fix routes through the new `CliError::SchemaParserFailure` variant.
- **D9-NEW-002 — `merge_allof` did not deduplicate `required` across branches**. The function concatenated each branch's `required` array via `.extend()`, producing duplicates when two branches independently required the same field name. Spec mandates first-seen-wins dedup (matching TypeScript `[...new Set(...)]` and Python's new explicit seen-set). Fix: replace `.extend()` with a `for item in req { if !merged_required.contains(item) { merged_required.push(...) } }` loop. Outer `obj.required` parent-vs-branches dedup at line 244-251 was already correct.

### Changed

- **`Makefile` `coverage` target** now passes `--fail-under-lines 85` to `cargo llvm-cov`, matching the Python `pyproject.toml` `[tool.coverage.report] fail_under = 85` and the new TypeScript `vitest.config.ts` `thresholds.lines: 85`. Cross-SDK CI parity (audit D5-004).

- **`apcli list` and `apcli describe` `--format` value-parsers** expanded to
  `[table, json, csv, yaml, jsonl, markdown, skill]`. `describe` previously
  accepted only `[table, json]`. Unknown values exit with code 2 (clap
  rejection) as before. Issue
  [aiperceivable/apcore-cli#20](https://github.com/aiperceivable/apcore-cli/issues/20).
- **Dependency bump**: `apcore = "=0.21.0"` (was `=0.19.0`) and the optional
  `apcore-toolkit = "=0.6.0"` (was `=0.5.0`). Aligns with upstream
  `apcore 0.21.0` (`Module::preview` / `PreflightResult::predicted_changes`)
  and `apcore-toolkit 0.6.0` (surface-aware formatters). No CLI-visible
  behavioural breaks.

### Added

- **`--format markdown` and `--format skill`** for `apcli list` and `apcli describe`
  (issue [aiperceivable/apcore-cli#20](https://github.com/aiperceivable/apcore-cli/issues/20)),
  gated behind the `toolkit` Cargo feature. Both delegate to
  `apcore_toolkit::format_module(s)` (≥0.6) so the output is byte-identical
  to the same toolkit call in the Python and TypeScript SDKs. `--format skill`
  emits vendor-neutral SKILL.md content directly loadable by Claude Code
  (`.claude/skills/<id>/SKILL.md`) and Gemini CLI
  (`.gemini/skills/<id>/SKILL.md`):

  ```bash
  apcore-cli apcli describe users.create --format skill > .claude/skills/users.create/SKILL.md
  ```

  A new internal `descriptor_to_scanned()` helper adapts the registry's JSON
  module-descriptor shape to the toolkit's `ScannedModule` type. When the
  `toolkit` feature is disabled, requesting `markdown` or `skill` logs a
  warning and falls back to `json`.
- **Issue #17 — `system_usage` aggregator + `list --sort calls|errors|latency`**:
  new module `src/system_usage.rs` reads `~/.apcore-cli/audit.jsonl`, filters
  by period (default 24h), and returns per-module aggregates (`calls`,
  `errors`, `avg latency_ms`). `list --sort {calls,errors,latency}` now
  consults the aggregator instead of falling back to id-sort with a buried
  `tracing::warn!`. When the audit log has no entries in the period window
  the discovery layer prints a user-visible note to stderr
  (`note: no usage data available for --sort <field>; sorted by id. ...`)
  and falls back to id-sort. Module-protocol registration of
  `system.usage.summary` / `system.usage.module` as registry-callable
  built-ins is tracked as a follow-up — today the readers are invoked
  directly by the discovery layer.
- New file: `src/system_usage.rs`.

- **Issue #18 + #19 — Rust parity**: new `pub fn create_cli_with(extensions_dir,
  prog_name, host_version, host_description) -> clap::Command` lives in the
  binary entry point (`src/main.rs`) — embedding API is BIN-only in v0.8
  pending the post-D9 redesign. `host_version=Some(v)` overrides
  `-V/--version`; `host_description=Some(d)` overrides the top-level
  `--help` "About" line. **Issue #18 opt-in semantics:** when
  `host_version` is `None`, `--version` is NOT registered — embedded callers
  no longer leak the SDK's own `CARGO_PKG_VERSION`. The standalone
  `apcore-cli` binary explicitly passes
  `Some(env!("CARGO_PKG_VERSION").to_string())` so its `--version` flag
  remains wired. When `host_description` is omitted, the surface defaults to
  `f"{prog_name} CLI"`. Rationale: the embedding API was removed in v0.7.0
  (D9-001/D9-002), but parameterizing the builder now means downstream Rust
  hosts experimenting with `apcore-cli` as a library do not have to fork
  the crate, and the re-introduced embedding API can route through this
  seam without further signature churn.
- **Issue #19 — debrand standalone help strings**: the top-level `--help`
  description, the `apcli` subgroup description, the `--verbose` option
  text, the root `after_help` footer, and the per-module verbose-hint footer
  in `cli.rs` no longer hard-code "apcore" in their phrasing. The
  description defaults to `f"{prog_name} CLI"` (matches TS / Python), and
  the four `(including built-in apcore options)` strings drop the trailing
  `apcore`. Standalone bin still uses the SDK package name for `prog_name`
  by default, so the public `apcore-cli --help` output is unchanged in
  spirit; downstream hosts now get a neutral surface out of the box.

### Removed

- **D9-003 — FE-13 §11.2 root-level deprecation shims**. The 13 hidden
  root-level shim subcommands (`list`, `describe`, `exec`, `validate`,
  `init`, `health`, `usage`, `enable`, `disable`, `reload`, `config`,
  `completion`, `describe-pipeline`) that forwarded to `apcli <name>` with a
  deprecation warning were removed per spec §11.3 ("Removed in v0.8").
  Callers must now use `apcli <name>`. The `DEPRECATED_ROOT_COMMANDS` const,
  `print_deprecation_warning`, `build_apcli_group_for_dispatch`,
  `forward_shim_args`, and `parse_shim_for` helpers in `src/main.rs` were
  deleted along with the registration loop and 13 dispatch arms.

---

## [0.7.0] - 2026-04-25

### Removed

- Removed `run_with_config` and `CliConfig` from the public surface — both
  were stubs and unwired (D9-001, D9-002). `run_with_config` returned 1 with
  a "not yet implemented" message in every branch; `CliConfig` declared
  `commands_dir`, `binding_path`, `group_depth`, `expose`, and `apcli`
  fields that no code path read. The embedding API will be reintroduced
  when actually implemented. `CliConfigError` was removed alongside.
- Removed `EXIT_CONFIG_NAMESPACE_DUPLICATE` constant alias (D9-003) — use
  `EXIT_CONFIG_NAMESPACE_RESERVED` for exit code 78.

### Added

- **Cross-language conformance test** (`tests/conformance_apcli_visibility.rs`) consuming the shared apcli-visibility fixtures from the `aiperceivable/apcore-cli` spec repo (`conformance/fixtures/apcli-visibility/`). One `#[test]` per canonical scenario (`standalone-default`, `embedded-default`, `cli-override`, `env-override`, `yaml-include`). Asserts apcli group visibility and subcommand registration against each fixture's `create_cli.json` / `env.json` / `input.yaml` inputs. A process-global `Mutex` guards scenarios that touch `APCORE_CLI_APCLI` / `cwd`. Byte-matching against `expected_help.txt` is gated behind `#[ignore]` until the canonical clap v4 / GNU-style help formatter is ported — tracked for parity with `apcore-cli-typescript/src/canonical-help.ts`.
- **`APCORE_CLI_SPEC_REPO` env var** — overrides the spec-repo lookup path for conformance fixtures. Defaults to a sibling checkout (`../apcore-cli/`). The test is a no-op (prints a skip notice and returns) when the spec repo is absent.
- New `[[test]]` entry in `Cargo.toml` registering the conformance test binary.
- **FE-12: Module Exposure Filtering** — Declarative control over which discovered modules are exposed as CLI commands.
  - `ExposureFilter` struct in `exposure.rs` with `is_exposed(&self, module_id)` and `filter_modules(&self, ids)` methods.
  - Three modes: `All` (default), `Include` (whitelist), `Exclude` (blacklist) with glob-pattern matching.
  - `ExposureFilter::from_config(value)` constructor for loading from `apcore.yaml` `expose` section.
  - `list --exposure {exposed,hidden,all}` filter flag in discovery commands.
  - `GroupedModuleGroup` integration: applies exposure filter during command registration.
  - `ConfigResolver` gains `expose.*` config keys.
  - 3-tier config precedence: `--expose-mode` CLI flag > env var > `apcore.yaml`.
    (The fourth `CliConfig.expose` tier was removed alongside `CliConfig` —
    see the Removed section above.)
  - Hidden modules remain invocable via `exec <module_id>`.
- New file: `exposure.rs`.

### Fixed

- Correctly propagate executor errors by moving `map_err` inside the `block_in_place` scope.

### Changed

- **CI — spec-repo checkout**: `.github/workflows/ci.yml` now checks out `aiperceivable/apcore-cli` into `.apcore-cli-spec/` and exposes it to `cargo test` via `APCORE_CLI_SPEC_REPO`. Mirrors the pattern in `apcore-cli-python` / `apcore-cli-typescript`.
- **Dependency bump**: requires `apcore = 0.18.0` (was `0.17.1`).
- `MAX_MODULE_ID_LENGTH` updated to 192 (was 128) — `cli.rs` constant `MODULE_ID_MAX_LEN` and
  `validate_module_id` already tracked the upstream spec change.
- `describe-pipeline` rendering updated to build a `StrategyInfo` value (new `apcore 0.18.0`
  type) from preset step data and use its `name` / `step_count` / `step_names` fields for
  display. Header format: `Pipeline: <name> (<n> steps)`.
- `FsDiscoverer::discover` signature updated to `discover(&self, _roots: &[String])` to match the
  new `apcore::registry::Discoverer` trait contract (`discover(roots: &[String])`).
- `Registry::discover(&discoverer)` now returns `usize` (module count) instead of
  `Vec<String>` — updated `main.rs` and `fs_discoverer.rs` tests accordingly.
- `Registry::get_definition` now returns `Option<ModuleDescriptor>` (owned) instead of
  `Option<&ModuleDescriptor>` — removed unnecessary `.cloned()` call in `discovery.rs`.
- Centralized CLI dispatch flags and builtin command definitions to improve maintainability.

---

## [0.6.0] - 2026-04-06

### Changed

- **Dependency bump**: requires `apcore = 0.17.1` (was `0.15.1`). Adds Execution Pipeline Strategy, Config Bus enhancements, Pipeline v2 declarative step metadata, `minimal` strategy preset.
- `CliConfig::group_depth` default changed from 0 to 1 (custom `Default` impl).
- Error tuple in executor path changed to `(i32, String, Option<Value>)` to carry structured error data for FE-11 enhanced error output.

### Added

- **FE-11: Usability Enhancements** — 11 new capabilities:
  - `--dry-run` preflight mode. Standalone `validate` command in `validate.rs` with `format_preflight_result()` and `first_failed_exit_code()`.
  - System management commands: `health`, `usage`, `enable`, `disable`, `reload`, `config get`/`config set` in `system_cmd.rs`. Graceful no-op when system modules unavailable.
  - Enhanced error output: `emit_error_json()` / `emit_error_tty()` with structured guidance fields from `Option<&Value>`.
  - `--trace` pipeline visualization with timing data.
  - `CliApprovalHandler` struct in `approval.rs`. `--approval-timeout`, `--approval-token` flags.
  - `--stream` JSONL output.
  - Enhanced `list` command: `--search`, `--status`, `--annotation`, `--sort`, `--reverse`, `--deprecated`, `--deps`, `--flat`. `ListOptions` struct.
  - `--strategy` selection: `standard`, `internal`, `testing`, `performance`, `minimal`. `describe-pipeline` command in `strategy.rs` with Pure/Removable/Timeout columns.
  - Output format extensions: `--format csv|yaml|jsonl`, `--fields` dot-path field selection. `format_module_list_with_deps()`.
  - Multi-level grouping: `CliConfig::group_depth`.
  - Custom command extension: `CliConfig::extra_commands: Vec<clap::Command>`.
- New error code constant: `EXIT_CONFIG_ENV_MAP_CONFLICT`.
- New files: `system_cmd.rs`, `strategy.rs`, `validate.rs`.
- `BUILTIN_COMMANDS` expanded to 14 entries. `KNOWN_BUILTINS` in `shell.rs` updated to match.
- `RESERVED_FLAG_NAMES` expanded with all FE-11 flag names.

---

## [0.5.1] - 2026-04-03

### Added
- **Pre-populated registry support** — `CliConfig` struct with optional `registry` (pre-populated `RegistryProvider`) and `executor` (pre-built `ModuleExecutor`) fields. When provided, downstream binaries can skip filesystem discovery entirely. This enables frameworks that register modules at runtime (e.g. apflow's bridge) to generate CLI commands from their existing registry.
- `CliConfig` exported from crate root with `Default` impl.

---

## [0.4.0] - 2026-03-29

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

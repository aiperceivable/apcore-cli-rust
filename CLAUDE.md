# CLAUDE.md — apcore-cli-rust

## Build & Test

- `make check` — runs fmt-check + clippy + all tests. **Must pass before considering any task complete.**
- `make build` — compile release binary to `.bin/`
- `cargo fmt --all` — format all code. **Run after every code change.**
- `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings required.
- `cargo test --all-features` — run all tests.

## Code Style

- All code must pass `cargo fmt --check` (rustfmt default: 100-column line width).
- Break long function signatures, macro calls, and chained method calls across multiple lines to stay within 100 columns.
- Follow existing naming patterns:
  - `_with_limit` / `_with_*` suffix for configurable-parameter variants that delegate from a simpler public API.
  - Public wrapper functions (e.g., `build_module_command`) delegate to `_with_limit` variants with default constants.
- `#[allow(dead_code)]` only when the field is intentionally kept for API symmetry.
- Error types use `thiserror::Error` derive.
- Tests live in `#[cfg(test)] mod tests` within each source file, plus integration tests in `tests/`.

## Project Conventions

- Spec repo (single source of truth): `../apcore-cli/docs/`
- Python reference implementation: `../apcore-cli-python/`
- All values in `ConfigResolver::DEFAULTS` are `&str` (not typed) — callers parse as needed.
- Exit codes are `pub const` in `lib.rs`, matching the protocol spec.
- `apdev-rs check-chars` is part of `make check` — no non-ASCII characters in source files.

## Environment

- Rust edition: 2021
- MSRV: 1.75+
- Async runtime: tokio
- apcore pinned exactly: `apcore = "0.21"` (v0.9.0 bump, was 0.19.0 at v0.7.0)
- Runtime schema validation: jsonschema 0.28
- apcore-toolkit-rust >= 0.7.0 — required runtime dependency as of v0.9.0; the `toolkit` Cargo
  feature flag is retained as a no-op for back-compat

## Current Conventions (v0.9.0)

- exposure module + ExposureFilter. Use `ExposureFilter::new` / `ExposureFilter::from_config` —
  the old `with_exposure_filter` builder was removed in v0.7.0 (D9-001/002).
- system_cmd module registers health/usage/enable/disable/reload/config commands (FE-11).
- strategy module + describe-pipeline + --strategy flag (FE-11).
- validate module + --dry-run flag (FE-11).
- 4 Config Bus exit codes: 65 (EXIT_CONFIG_BIND_ERROR), 66 (EXIT_CONFIG_MOUNT_ERROR),
  70 (EXIT_ERROR_FORMATTER_DUPLICATE), 78 (EXIT_CONFIG_NAMESPACE_*).
- `CliApprovalHandler` struct is a configuration holder (stores `auto_approve` and `timeout`).
  Actual approval gating is performed by standalone `approval::check_approval` / `check_approval_with_tty` functions.
- `Sandbox::execute(&self, module_id, input, executor)` — 3-parameter async signature, identical
  in arity to Python `sandbox.execute(module_id, input_data, executor)` and TS
  `sandbox.execute(moduleId, inputData, executor)`. The Rust sandbox does additionally
  bind APCORE_EXTENSIONS_ROOT at construction time via `withExtensionsRoot`, but the
  per-call executor is still passed in. (Earlier docs claimed a 2-param form — that
  divergence no longer exists, audit D10 follow-up 2026-05-13.)
- `set_all_options_help(bool)` / `is_all_options_help()` — Rust uses the v0.9.0 renamed form;
  Python/TS still export `set_verbose_help` (internal name retained for back-compat).
- `register_pipeline_command(cli) -> Command` — no executor param; differs from Python/TS form.
  See builtin-group.md for the Rust language note.
- `resolve_refs(schema, defs)` — 2-parameter form; defs extracted externally. Differs from
  Python/TS 3-param form. See schema-parser.md for the Rust language note.
- ApcliGroup has 6 constructors (`from_cli_config`, `from_cli_config_with_name`, `from_yaml`,
  `from_yaml_with_name`, `try_from_yaml`, `try_from_yaml_with_name`) vs Python/TS 4 (name
  is a kwarg). The `_with_name` variants expose the FE-13 `builtin_group_name` feature.
- csv/jsonl/markdown/skill output formats are toolkit-delegated (ADR-09, v0.9.0).
- No top-level `create_cli` factory — see `src/lib.rs:142-178` for the documented embed-API
  parity gap. Pending rebuild.
- schemars in [dev-dependencies] only (used in examples/).

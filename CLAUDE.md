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
- apcore pinned exactly: `apcore = "=0.17.1"` (v0.6.0 bump, was 0.15.1)
- Runtime schema validation: jsonschema 0.18 (added in v0.6.0 for Py/TS parity)
- Optional: apcore-toolkit >= 0.4 behind the `toolkit` feature flag

## v0.6.0 Conventions

- exposure module + ExposureFilter + `with_exposure_filter` builder pattern on the
  grouped command group (FE-12). Note: Rust CliConfig does NOT yet expose an `expose`
  field directly — filter must be wired via the builder method on the command group.
- system_cmd module registers health/usage/enable/disable/reload/config commands (FE-11).
- strategy module + describe-pipeline + --strategy flag (FE-11).
- validate module + --dry-run flag (FE-11).
- 4 Config Bus exit codes added: 65 (EXIT_CONFIG_BIND_ERROR), 66 (EXIT_CONFIG_MOUNT_ERROR),
  70 (EXIT_ERROR_FORMATTER_DUPLICATE), 78 (EXIT_CONFIG_NAMESPACE_*).
- `CliApprovalHandler` struct is currently a configuration holder only (stores
  `auto_approve` and `timeout`). Actual approval gating is performed by standalone
  `approval::check_approval` / `check_approval_with_tty` functions. Full trait-method
  implementation is tracked as apcore-skills:sync finding A-001.
- `Sandbox::execute()` currently needs executor wiring — tracked as A-003.
- Public surface hygiene: `dispatch_*`/`register_*`/command builder fns should be
  `pub(crate)` not `pub` — only called from main.rs. Ongoing cleanup per D9-005.
- schemars moved to [dev-dependencies] (v0.6.0 cleanup) — used only in examples/.

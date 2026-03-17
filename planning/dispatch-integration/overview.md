# dispatch-integration — Feature Overview

> Wire dynamic module dispatch into the CLI, register the exec subcommand, and
> address remaining suggestion-level code quality items from the project review.

---

## Scope

**Included:**
- Register `exec` as a first-class clap subcommand
- Wire external subcommand match arm to `dispatch_module`
- Change `resolve_refs` signature to `&Value`
- Make `validate_extensions_dir` return `Result` instead of `process::exit`
- Gate `MockRegistry` behind `#[cfg(test)]`
- Remove unused test helpers and imports

**Excluded:**
- `serde_yaml` migration (tracked separately)
- New feature development

## Technology Stack

- Rust 2021, clap v4, tokio, apcore 0.13.0
- Testing: `cargo test` (TDD)

## Task Execution Order

| # | Task File | Description | Status |
|---|---|---|---|
| 1 | [exec-subcommand](./tasks/exec-subcommand.md) | Register exec subcommand in clap tree | pending |
| 2 | [validate-extensions-result](./tasks/validate-extensions-result.md) | validate_extensions_dir returns Result | pending |
| 3 | [resolve-refs-signature](./tasks/resolve-refs-signature.md) | Change &mut Value to &Value in resolve_refs | pending |
| 4 | [test-cleanup](./tasks/test-cleanup.md) | Gate MockRegistry, remove unused helpers/imports | pending |
| 5 | [dispatch-wiring](./tasks/dispatch-wiring.md) | Wire external + exec match arms to dispatch_module | pending |

Tasks 1-4 are independent and can run in parallel. Task 5 depends on tasks 1 and 2.

## Progress

```
[░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░] 0 / 5 tasks complete (0%)
```

## Reference Documents

- [Feature spec](./feature-spec.md) — requirements from review consistency gaps
- [Project overview](../overview.md) — project-level dashboard

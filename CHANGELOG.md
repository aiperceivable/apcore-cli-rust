# Changelog

All notable changes to this project will be documented in this file.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).


## [0.12.0] - 2026-09-06

Two features: **FE-14 ACL Governance** makes the CLI's access-control surface reachable for the first time, and **FE-15a OpenAPI Import** adds `apcli openapi scan` / `generate`. `APCLI_SUBCOMMAND_NAMES` grows from 13 to 15.

The ACL work turned up **two silent access-control bypasses** — see Security below. They were present in all three SDKs and are fixed in all three.

`make check` is green end to end: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (0 warnings), `apdev-rs check-chars` (29 files, ASCII-only), and 996 tests across 33 binaries, up from 800.

### Added

- **FE-14: the CLI now attaches an ACL.** apcore has enforced access control since PROTOCOL_SPEC §6, and this CLI has always carried the *downstream* half of it — exit code `77` for `ACL_DENIED`, an `acl` row in `--dry-run` preflight, `acl_check` in `apcli describe-pipeline`. None of it was ever reachable, because **no apcore-cli SDK had ever constructed or attached an `ACL`**: all three build an `Executor` directly rather than going through the `APCore` bootstrap that performs `ACL::discover`. The result was an executor whose `acl_check` step consulted nothing, and a `governance_state()` reporting `unprotected_control_surface: true` for every project — including projects shipping an `acl/global_acl.yaml` and reasonably assuming it was in force.

  New `src/acl_loader.rs` resolves an ACL root through the FE-07 4-tier chain (`--acl` > `APCORE_ACL_ROOT` > `acl.root` in `apcore.yaml` > `./acl`) and delegates the parse to `ACL::load`. Rule-key closure, `effect` / `approval` enum closure and pattern-array arity are apcore's contract and conformance-tested there; the CLI does not reimplement them.

  Enforcement is **only-when-configured**. A missing root attaches nothing and changes no behaviour, preserving apcore's missing-path invariant: synthesizing an empty ACL with `default_effect: deny` would deny every call in every project that lacks an `acl/` directory. Every existing project behaves exactly as it did.

- **`apcli acl` subcommand group** (`src/acl_cmd.rs`): `list`, `check`, `validate`, `status`.

  `check` calls `ACL::check_access`, never the boolean `ACL::check` — the latter fails closed on an approval requirement, returning `false` for a call that is *allowed but needs a human*, which would report "denied" for a rule set that in fact permits the call. Both axes are reported separately, and an allow-with-approval outcome exits `0`.

  `validate` renders `sync` and `async` as separate columns rather than one boolean: a finding with `sync=no, async=yes` is an async-only handler, working under `async_check()` and unevaluable under `check()`, and collapsing them loses exactly that.

  `status` renders all nine `Executor::governance_state()` observations. `acl_configured` alone is not the answer — the ACL and approval gates are pipeline *steps*, and `internal` / `testing` / `minimal` remove them, so an executor can hold an ACL that no step ever consults.

- **`--identity-id` / `--identity-type` / `--role` global flags.** These build a `Context` identity so conditional rules keyed on `roles` or `identity_types` are evaluable from the terminal. They are **unauthenticated argv assertions, not authentication**, and each flag's `--help` says so. `apcli acl check` restates the three with identical wording; clap resolves the two levels per argument, so a subcommand flag overrides only its own counterpart and a root flag not restated still applies.

  `Context::caller_id` is never fabricated — apcore deliberately makes it unsettable, so a top-level CLI call is always `@external`, and a flag that set it would let any user assume any module's identity. When only `--role` or `--identity-type` is given, `Identity.id` falls back to `DEFAULT_IDENTITY_ID` (`@cli`), pinned in value and export name across all three SDKs; the `@` prefix follows apcore's synthetic-principal convention so it cannot collide with a real user id of `cli`.

- **FE-15a: `apcli openapi scan` and `apcli openapi generate`** (`src/openapi_cmd.rs`, `src/openapi_source.rs`). `scan` reads an OpenAPI 3.0/3.1 document through the toolkit's `OpenAPIScanner` and renders the modules it would produce in every FE-08 format; `generate -o DIR` materializes them as `<id>.binding.yaml` through `YAMLWriter`. Neither registers a module, builds an executor, or issues a request to the described API — `scan` of a local file performs no network I/O at all.

  The CLI is an adapter, not a second implementation: `derive_module_id` output is returned verbatim (it is the subject of a cross-SDK conformance corpus and must match byte-for-byte in three languages), schema extraction is the toolkit's, and the routing contract is exactly the two flat keys `http_method` and `url_path`. The scanner hooks are deliberately not exposed as flags — overriding derivation hands back the naming guarantee, which is not something a command-line flag should be able to do silently.

- **Proxy-hazard detection.** `HTTPProxyRegistryWriter` decides body-versus-query by HTTP method alone, so a query parameter declared on a `POST` / `PUT` / `PATCH` operation **would be sent in the request body** — silently. FE-15a cannot fix that (the fix is upstream, in apcore-toolkit 0.12.0), but it makes it visible: the CLI holds the raw document, which still carries `parameters[].in`, so affected operations are named with their method and offending parameter names by both `scan` and `generate`. Hazards are counted separately from scanner warnings, appear under a top-level `hazards` key in machine formats because they describe a *future* execution path, and never change the exit code.

- **FE-14 §4.8: ACL decisions now reach the FE-05 audit log.** apcore emits exactly one `AuditEntry` per `check_access()` call, but only through an `audit_logger` callback, and nothing in apcore wires the `acl.audit.*` keys to one. The CLI now does: when `acl.audit.enabled` is true, the same `AuditLogger` the module-dispatch path uses is installed as the callback, so ACL decisions land in `~/.apcore-cli/audit.jsonl` beside execution records.

  **This SDK attaches with `ACL::set_audit_logger` rather than rebuilding.** §4.8 describes `ACL::new(src.rules, src.default_effect, logger)` because that is the only mechanism Python and TypeScript offer, and explicitly permits an SDK to use whichever its runtime has. Rust's setter is strictly less lossy on two counts: the rebuild must carry `default_effect` across by hand — pass a literal `"deny"` and every file declaring `default_effect: allow` has its governing default silently inverted for each unmatched call — and the rebuild drops the `yaml_path` that `reload()` depends on. The setter can express neither mistake. `acl.audit.enabled: false` attaches the `ACL::load` result with no callback and no rebuild; an ACL an embedder supplied itself never reaches the loader and is attached unchanged.

  **The wire record is 13 fields, in apcore's `AuditEntry` declaration order, and nothing else.** Key order is normative — the log is JSONL, so an unspecified order would make the same decision serialize to different bytes per SDK. It is pinned by a `#[derive(Serialize)]` struct rather than a `serde_json::json!` literal: `serde_json::Map` is a `BTreeMap` unless the `preserve_order` feature happens to be enabled somewhere in the dependency graph, so a map literal would emit alphabetical order on one build and insertion order on another, silently. Serialising apcore's `AuditEntry` directly is wrong for a second reason — `skip_serializing_if = "Option::is_none"` on six optional fields would drop `matched_rule`, `matched_rule_index`, `identity_type`, `call_depth`, `trace_id` and `handler_error` from any entry that did not populate them. Here an absent value is `null` and every line carries the same key set. No CLI field is added either, notably not the `user` field FE-05 puts on *execution* records, so a consumer can read an ACL record against apcore's `AuditEntry` rather than a CLI dialect of it.

  `acl.audit.include_denied` governs **denied** decisions, matching apcore's own `schemas/acl-config.schema.json` ("Whether to log denied access attempts"): `false` suppresses deny entries and leaves allow entries alone. It is not an inverted "log denials only" switch.

  A logging fault never changes an access decision. The callback is infallible by construction — building the record cannot fail, and `AuditLogger` swallows its own IO errors behind a one-shot warning — so an unwritable audit log costs the entry and nothing else.

- **Test coverage.** `tests/test_acl_cmd.rs` (70 cases, including the §4.8 rows T-ACL-26 / 27 / 27a / 27b / 27c) and `tests/test_openapi_cmd.rs` (40 cases), driven end-to-end through the real binary so the asserted exit codes are the ones a user's shell sees. The section 4.10 cases use a sentinel file the module's own script creates, because an exit code alone does not prove a subprocess was never started.

  The §4.8 rows run in-process instead, deliberately: the production audit path writes to `~/.apcore-cli/audit.jsonl`, so a test spawning the real binary with auditing on would append to the developer's own log. They call the exact function `main.rs` calls, with an `AuditLogger` pointed at a temp file. T-ACL-27a asserts that **no logger was installed** — read off apcore's `ACL` `Debug` rendering, the only introspection it offers — rather than merely that no entries were written; the two differ, and only the former rules out a callback that silently drops everything. T-ACL-26 asserts an **equality** on the ordered key list read off the raw JSONL text, which pins field set, order, casing and the absence of extras at once.

### Changed

- **`apcore = ">=0.30"`, `apcore-toolkit = { version = ">=0.11.1", features = ["http-proxy"] }`.** The floors track the aligned apcore 0.30.0 / apcore-toolkit 0.11.1 release. Both bumps are confined to layers this CLI does not consume, so neither forced a behavioural change here — the only edit either required was to a test that had pinned the toolkit dependency line *verbatim*, version floor included, and therefore failed on any bump regardless of what changed. It now asserts the `features = ["http-proxy"]` half, which is what it was written to guard; the floor is Cargo's business.

  The `http-proxy` feature is required, not optional: `load_spec` sits behind it in the Rust toolkit, and an SDK that cannot reach the HTTP path must fail with an actionable message rather than a missing-symbol link error. Local-file scanning and the YAML writer need none of it.

- **`ConfigResolver::DEFAULTS` gains `acl.root` (`./acl`), `acl.audit.enabled` (`true`) and `acl.audit.include_denied` (`true`).** All three are apcore-owned keys, so their environment variables are `APCORE_ACL_ROOT`, `APCORE_ACL_AUDIT_ENABLED` and `APCORE_ACL_AUDIT_INCLUDE_DENIED` — the apcore convention, following the `APCORE_EXTENSIONS_ROOT` precedent rather than extending it. There is deliberately **no `acl.enabled: false`**: a key whose only effect is to silently disable access control is a foot-gun that reads as configuration. To disable enforcement, point `acl.root` at a path that does not exist.

  The two audit booleans accept `true`/`1`/`yes`/`on` and `false`/`0`/`no`/`off`, case-insensitively after trimming — a table shared with Python and TypeScript, and deliberately not `str::parse::<bool>()`: Rust's `FromStr for bool` *errors* on `"0"`, so delegating to it would leave `APCORE_ACL_AUDIT_ENABLED=0` unable to switch auditing off while the same value worked in the other two SDKs. An unrecognised spelling falls back to the key's default (`true`) with a warning naming the key, rather than to `false` — reading an unparseable governance value as "off" would let a typo silently stop the audit trail.

- **`tests/acl_argument_scoped_approval.rs` builds its rules through `ACLRule::new`.** apcore 0.29.0 makes `ACLRule` `#[non_exhaustive]`, so struct-literal construction no longer compiles across the package boundary.

### Fixed

- **`ACL_RULE_ERROR` exited 1 instead of 47.** It is a real `apcore::errors::ErrorCode` that no SDK's exit map carried, so a malformed ACL file fell through to `map_apcore_error_to_exit_code`'s catch-all arm — the code that reads as "the module ran and failed", indistinguishable from a genuine execution failure. Now `47` (CONFIG_INVALID), added in all three SDKs together.

  `47` rather than `77` because the ACL could not be *read*, which is a configuration fault, not a denial. `77` stays reserved for an actual access decision, or a script branching on it would misreport a broken config as a permissions problem.

- **The test suite appended to the developer's real `~/.apcore-cli/audit.jsonl`.** `tests/test_e2e.rs` spawns the binary without `APCORE_CLI_AUDIT_DISABLE=1`, so three `math.add` executions wrote 639 bytes of real audit records into the developer's own log on every `cargo test` run — and `system_usage`'s summary reader, which derived the same home path independently, read from it. Pre-existing in all three SDKs and fixed in all three; the FE-14 §4.8 work is what surfaced it. It never made a test fail, so a green suite was never evidence either way.

  Under the existing `test-support` feature, `AuditLogger::default_path()` now resolves to a temp file instead. The fix is deliberately **not** `APCORE_CLI_AUDIT_DISABLE=1` in `test_e2e.rs`: that would stop those tests exercising the audit path at all, trading a visible problem for an invisible one. Redirecting keeps the writes happening where they can be counted. The redirect path is *derived* rather than published through an environment variable, so the test process and every binary it spawns compute the same value independently — `std::env::set_var` is unsound once other threads are running, and test binaries are multi-threaded by default.

  A production build compiles the branch out entirely (verified: the string `apcore-cli-test-audit` does not appear in the `cargo build --release` binary), so no environment or feature setting can relocate a released binary's audit trail. `home_audit_path()` is split out and pinned by its own test against the real home-derived value, because an assertion that `default_path()` merely "contains `.apcore-cli/audit.jsonl`" would keep passing against the redirect and silently stop testing the shipped location. `system_usage` now delegates to the same function rather than re-deriving the path, so reader and writer cannot drift apart.

  Verified by measurement, not by tests passing: `~/.apcore-cli/audit.jsonl` held 241911 bytes / md5 `75d0c8f5e1cd2951436379cd776a0bc7` both before and after a full `cargo test --all-features`, while the redirect target captured the 3 records (642 bytes) that used to land there — confirming the writes moved rather than merely stopped.

- **Tracing wrote diagnostics to stdout, corrupting every machine format.** apcore emits one WARNING per unevaluable ACL rule at the default log level, and the `tracing_subscriber` fmt layer's default writer is stdout — so `apcli acl validate --format json | jq` failed on a rule set that had anything to report, which is precisely when a user runs it. The layer now writes to stderr, where every other diagnostic in this crate already goes.

### Security

- **`--sandbox` silently disabled access control.** `sandbox_runner` constructs a fresh `Registry` + `Executor` from `APCORE_EXTENSIONS_ROOT` with **no ACL attached**, so a rule set that denied a module was enforced for a plain call and ignored for a sandboxed one. This inverts the user's intent outright: `--sandbox` is a *security* flag, so switching on stronger isolation switched off access control. Present in Python and TypeScript too, fixed in all three.

- **Filesystem script modules were never gated.** `FsDiscoverer` executables are spawned as subprocesses and never reach `Executor::call`, so the pipeline's `acl_check` step never saw them — meaning a configured ACL was silently ignored for exactly the modules this CLI discovers. An operator writing `acl/global_acl.yaml` would believe a denial was in force when nothing was checking. Rust-specific (the other SDKs have no equivalent discovery path).

  Both are the same defect: attaching an ACL to the executor gates the calls that go *through that executor*, and gates nothing else. The decision is now reached in the **parent**, which already holds the ACL, and a denied call is refused with exit `77` **before the subprocess is spawned** — one enforcement point rather than one per execution mechanism. The child re-loading `acl.root` is explicitly not the control: the sandbox forwards a narrow environment allowlist by design, so the child's view is neither guaranteed nor trustworthy as a gate.

  An ACL-sourced `approval: required` composes with the module annotation before the CLI's approval gate on these paths too, exactly as apcore's gate does for a normal call — otherwise the same rule would demand a human on one path and wave the call through on another. Five tests go red when the gate is reverted, including the pair that proves the sentinel file is not created.

- **The gate itself had the same bypass one level down: it passed no `Context`.** A `Context` is built from the identity flags and is legitimately `None` when none were given — correct for `apcli acl check`, which *simulates* a call and is honestly context-free. It is wrong for a gate. PROTOCOL_SPEC §6.5 makes every conditional rule a non-match when a call supplies no context, while apcore's pipeline creates one at Step 1 for **every** real call. So a `deny` rule carrying `conditions` fired in-process and went inert on the delegated path — the same silent bypass, one level down, and invisible to any test using an unconditional rule.

  Both gates now always present a context: the identity-bearing one when flags were given, otherwise a freshly built `@external` / `external` identity reproducing exactly what `Executor::call` constructs for `ctx: None`, so an `identity_types` rule behaves identically on both paths. They also pass the call's arguments as the governance projection, without which an `arguments`-scoped rule is unevaluable — and per §6.1.1 an unevaluable `deny` rule *takes effect*, so the omission silently denied calls it should have permitted as well as permitting ones it should have denied. Both directions are pinned by discriminating pairs.

- **Credentials never reach disk.** Headers supplied via `openapi generate --header` to fetch a protected document exist only for that fetch; neither they nor the document's `securitySchemes` are copied into any generated artifact.

### Notes

- **A retracted claim, recorded because it reached implementers.** An earlier draft of this changelog — and of FE-14 §4.8 — said the audit wiring was blocked on a public `ACL.set_audit_logger` that Python and TypeScript would have to gain, and that shipping it in Rust alone would put this SDK ahead on a cross-SDK surface. That was wrong on the premise: **all three SDKs already accept the callback as a constructor argument** (`ACL(rules, default_effect, audit_logger=…)`, `new ACL(rules, defaultEffect, auditLogger)`, `ACL::new(rules, default_effect, audit_logger)`), so §4.8's load-then-construct sequence needed nothing new anywhere. Rust's `set_audit_logger` is an extra convenience on top, not the prerequisite. The wiring and both `acl.audit.*` keys land here in 0.12.0, alongside Python and TypeScript.

- **All of FE-15b is excluded.** `generate` produces binding artifacts; it does **not** make an API callable, and the commands' `--help` says so rather than implying otherwise. Passing the generated files to `--binding` does not yet produce working commands, on two independent prerequisites: `--binding` is a real registration path only in Python (TypeScript populates a display-overlay map; Rust constructs a `DisplayResolver` and discards it), and `HTTPProxyRegistryWriter` cannot correctly encode a query parameter declared on a body method until apcore-toolkit 0.12.0 carries parameter locations. Neither is about OpenAPI; both are pre-existing debt.

- **`--writer native` was specified and then withdrawn.** Every toolkit source writer resolves `ScannedModule.target` as a `module.path:callable` import path, while an OpenAPI-derived `target` is always a route descriptor such as `"GET /pets"` — so the flag could never have succeeded for any input `generate` can produce. Same root cause as the `RegistryWriter` limitation. `generate` is binding-YAML only, and no refusing stub is left behind. Emitting genuine host-language source for an OpenAPI operation means emitting an HTTP proxy implementation, which belongs with FE-15b.

- **`apcli acl` is a `requires_executor` entry but is not in `APCLI_ALWAYS_REGISTERED`** — under `mode: include` it registers only when explicitly listed. `openapi` needs neither registry nor executor. Neither is a system command, so neither gates on `system.health.summary` availability.

- **The `apcli-visibility` golden byte-match remains `#[ignore]`d** pending the canonical help formatter port, as it has been since FE-13. All five behavioural scenarios pass. The four new root flags' help strings are normative across all three SDKs, and are pinned here by a direct unit test on the clap `Arg` metadata rather than only through the golden — a fixture that does not byte-match in every SDK would let a reword pass locally and break the others.


## [0.11.0] - 2026-09-02

Bumps the required `apcore` floor to `0.28` and `apcore-toolkit` to `0.10.2` to track the aligned apcore 0.28.0 release (2026-08-31). Carries one display fix and a crate-root lint attribute (both under Fixed). `make check` is green end to end: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (0 warnings), and 800 tests across 31 binaries including the conformance suite, all against apcore 0.28.0.

**Why a minor rather than a patch.** This SDK needed no correctness fix of its own, but the three CLI SDKs ship as one version line, and the `apcli health` summary output changes here too. Version-locking them is what keeps the compatibility table and the cross-SDK conformance fixtures meaningful; see apcore-cli-python 0.11.0 for the changes that set the bump. Note for downstream crates: a `apcore-cli = "0.10"` pin does not resolve 0.11 — widen it to `">=0.11"` or `"0.11"`.

### Added

- **`tests/acl_argument_scoped_approval.rs` (4 cases)** pins the cross-SDK contract that an ACL-sourced approval requirement reaches `CliApprovalHandler`, replacing what had been a code-path argument with a measurement. The fourth case is the discriminating one — with auto-approve off and no TTY under `cargo test` the handler refuses, so the ungated call must still succeed while the `force`-carrying call must fail with `ErrorCode::ApprovalDenied`. Without that pair a gate that never fired would pass the suite.

### Changed

- **`apcore = ">=0.28"`, `apcore-toolkit = ">=0.10.2"`.** apcore-toolkit 0.10.2 is a dependency-tracking release with no source change.

### Fixed

- **`DEPENDENCY_NOT_FOUND` and `DEPENDENCY_VERSION_MISMATCH` exited 1 instead of 44.** Both are real `apcore::errors::ErrorCode` variants, and both reached `map_apcore_error_to_exit_code`'s catch-all arm, which returns `EXIT_MODULE_EXECUTE_ERROR`. apcore-cli-python and apcore-cli-typescript map both to 44, so the same dependency failure ended a script with a different code depending on which CLI ran it — and 1 is the code that reads as "the module ran and failed" rather than "the module could not be resolved".

  Found by a mechanical three-way diff of the exit-code maps, not by inspection: extracting all three and comparing key-for-key reported **2 divergent of 22 codes**, and re-running it after the fix reports 0. `EXIT_DEPENDENCY_NOT_FOUND` / `EXIT_DEPENDENCY_VERSION_MISMATCH` are named as their own constants to mirror the other two SDKs rather than reusing `EXIT_MODULE_NOT_FOUND` at the call site. Pinned in all three SDKs so the maps cannot drift again.

- **The `apcli health` summary line reported "no data" for a project whose modules it had just listed.** apcore classifies module health in **four** tiers — `healthy` / `degraded` / `error` / `unknown` — and the tally iterated only the first three. `unknown` means "no calls recorded yet", which is the state every module in a fresh project is in, so the common case rendered a populated table above a total that denied it:

  ```
    probe.echo                   unknown      0.0%         --
  Summary: no data
  ```

  **Pre-existing, and not introduced by this upgrade** — all three SDKs have emitted `unknown` since the tier set existed. apcore 0.28.0 is what brought it into focus: `sys-health-summary.schema.json` had declared the enum as `["healthy", "degraded", "unhealthy"]`, a value **no SDK emits**, and the release corrects it to the four tiers actually produced, splitting the summary's `unhealthy` count field into `error` and `unknown`. With the canonical shape finally naming four tiers, rendering three is a plain omission. Fixed in all three SDKs together, with the tally now covering `unknown`; a genuinely empty tally still reads "no data".

- **`cargo clippy -- -D warnings` failed on `clippy::result_large_err`, which would have taken CI red.** Two `Sandbox` methods returning `Result<Value, ModuleExecutionError>` were flagged because the enum's passthrough variant carries `apcore::errors::ModuleError` by value and that type is ≥184 bytes. **Not a regression from this upgrade** — reproduced identically with the previous `apcore = ">=0.27"` pin, so the trigger was the clippy version, not apcore; but CI runs `dtolnay/rust-toolchain@stable` with `-D warnings`, so it was going to fail there regardless of when it started.

  Resolved with a crate-root `#![allow(clippy::result_large_err)]`, mirroring apcore-rust, which suppresses the same lint at its own crate root with the reason that applies verbatim here: *"ModuleError is intentionally large (rich structured error for an SDK); boxing it everywhere would change the public API."* The variant holds the error by value on purpose — `cli::map_module_error_to_exit_code` reads its `ErrorCode` to keep the exit-code taxonomy identical across the `--sandbox` and direct paths — so boxing would both break a public enum in a patch release and diverge from the decision made by the crate that owns the type.

- **The discriminating approval tests no longer depend on stdin not being a terminal.** They originally used `CliApprovalHandler` with auto-approve off and relied on its non-TTY refusal, which is not a property of the test — it is a property of how the suite happens to be launched. `cargo test` does not redirect stdin, so run from an interactive shell the Rust case printed its prompt, blocked for the full 60-second timeout and then failed on `ApprovalTimeout` instead of `ApprovalDenied`; `pytest -s` and a main-thread vitest configuration reach the same trap. All three now register a small recording stub that always refuses, which removes the ambient dependency and lets each test assert the stronger property directly: that the gate consulted a handler **at all**, and for exactly which call. The real `CliApprovalHandler` is still exercised, on the auto-approve path, where its answer is deterministic.

### Notes

- **Three of 0.28.0's BREAKING Rust changes land on types this crate names, and all three are source-compatible here.** `ACLRule` gains an `approval` field and `AuditEntry` becomes `#[non_exhaustive]` — neither is constructed by this crate, which never builds or loads an `ACL`. `CallbackApprovalHandler::new` now takes an async fallible callback — `CliApprovalHandler` implements `apcore::ApprovalHandler` directly and never uses the convenience constructor. `ACL::evaluate_conditions` returning `ConditionOutcome` and `ACL::check` failing closed on allow-with-approval-required are both unreachable: the crate calls neither.

- **The one 0.28.0 change that reaches this SDK works correctly and needed no code.** Spec v1.28.0 §6.9 makes the approval gate fire on the union of three sources, so an ACL rule carrying `approval: required` (§6.1.6) now routes calls to modules annotated `requires_approval: false` through `CliApprovalHandler`. The trait adapter rebuilds its `module_def` shape from `request.annotations.requires_approval`, and `builtin_steps.rs:816` sets that to `true` before constructing the request for **any** source of the requirement — so `get_requires_approval` passes and the prompt runs. `cli_to_apcore_result` already returns a typed `apcore::ApprovalResult`; apcore-cli-python returned a bare mapping on the same path and had to be fixed in its 0.11.0.

- **`system.usage.*` behaviour changes are upstream-side and pass through unread.** 0.28.0 makes both modules honour `period` (statistics were previously computed over the full retained history) and changes `hourly_distribution[].hour` to `YYYY-MM-DDTHH`. `dispatch_usage` forwards `--period` verbatim, and the two TTY formatters read only `modules`, `period`, `module_id`, `call_count`, `error_count`, `avg_latency_ms`, `trend` and `p99_latency_ms`. `hourly_distribution` appears nowhere in this crate — nor in the Python or TypeScript CLIs — so no formatter or assertion depends on the retired key shape.

- **What the delta does *not* touch.** `Registry.list` / `get_definition`, `Executor::call` / `validate`, the approval handler protocol and the toolkit `format_*` surfaces are all unchanged across 0.27.0 → 0.28.0.


## [0.10.5] - 2026-08-17

Patch release. Bumps the required `apcore` floor to `0.27` to track the aligned apcore 0.27.0 release (2026-08-14). **No source changes** — the full test suite (fmt + clippy + all tests) passes unchanged against apcore 0.27.0, including the 511-case conformance suite.

The apcore 0.26.0 → 0.27.0 delta is BREAKING at the spec level, but touches no surface the CLI consumes — verified against the release notes and the actual call sites:

- **Middleware semantics** — `before_step` failure is now terminal/non-recoverable, `after_step` fires after a recovered step body. The CLI never constructs or configures middleware or pipelines; it only constructs `Executor::new(Arc<Registry>, Config)` and calls `Executor::call(module_id, input, None, None)` / `describe_pipeline` (read-only `StrategyInfo`). No exposure.
- **ACL-failed `validate()` introspection** — a failed `acl` check now withholds `module_preflight` / `module_preview` checks and `predicted_changes`. The CLI's validate path builds its own preflight checks locally from the registry descriptor (`validate.rs`), never consuming `Executor::validate()`'s return shape. No exposure.
- **`Registry.register_versioned` metadata `dependencies` persistence** — the CLI never calls `register`; module registration is via `Registry::discover(&FsDiscoverer)`, which emits `dependencies: vec![]` / empty `metadata`. No exposure.
- **Schema conversion (A23)** — object detection, nullable `anyOf` wrapping, sorted `required` are SDK-conversion rules. The CLI runs its **own** schema→clap converter (`schema_parser.rs` / `ref_resolver.rs`) on the descriptor's `input_schema`; `required` is order-insensitive and `type`-less nullable branches already fall to the string default. No exposure.
- **`pipeline.configure` 4-field set / `requires`/`provides` non-configurable** — the CLI never configures pipelines; a host config carrying other keys now fails at load (spec-mandated strictness, upstream concern).
- **No type coercion at the module boundary** — the CLI's own clap-string→JSON coercion (`cli.rs`) applies before `call()`, which receives already-typed JSON values. No exposure.
- **Removed/renamed API surface** — `ErrorCode::ConfigurationError` → `PipelineConfigurationError`, OtelTracing* removal + `opentelemetry` feature, `TracingMiddlewareConfig` field removal, `SchemaValidator::new()` no-coerce default — none used by the CLI (which uses only the `ModuleExecuteError` variant and no tracing middleware).

## [0.10.4] - 2026-07-14

Patch release. Bumps the required `apcore` floor to `0.26` to align the ecosystem on the 0.26.0 governance layer (additive, no breaking changes). No code or API changes.

## [0.10.3] - 2026-07-07
update package dependency version for apcore-toolkit (0.10.0) and increment project patch version

## [0.10.2] - 2026-06-24

### Changed

- **Required runtime bumped to apcore 0.25.0 and apcore-toolkit 0.9.1.** `Cargo.toml`
  dependencies raised from `apcore = "0.24"` / `apcore-toolkit = "=0.8.1"` to
  `apcore = "0.25"` / `apcore-toolkit = "=0.9.1"`, tracking the aligned apcore 0.25.0
  and apcore-toolkit 0.9.1 releases (both resolve from crates.io). **No source
  changes** — the full test suite passes unchanged.

  Neither delta touches a surface the CLI consumes:
  - **apcore 0.24.0 → 0.25.0** adds config-driven ACL discovery (`acl.root`
    activation + `ACL.discover`), auto-wired only by the `APCore` bootstrap and
    skipped when the caller supplies its own `Executor`. The CLI builds its own
    `Executor::new(Arc<Registry>, config)` and never constructs `APCore`, so
    discovery does not engage. The companion change — Rust's `acl.root` now
    defaulting to `./acl` instead of being hard-required — only relaxes config
    validation and is backward-compatible.
  - **apcore-toolkit 0.8.1 → 0.9.1** is a bug-fix release; its only API-surface
    change relaxes `RegistryWriter::write` / `HttpProxyWriter::write` from
    `&mut Registry` to `&Registry` (source-compatible, and unused by the CLI). The
    toolkit surface the CLI uses (`format_*`, `DisplayResolver`, `ScannedModule`,
    `ModuleStyle`, `FormatOutput`) is unchanged.

## [0.10.1] - 2026-06-15

### Changed

- **Required runtime bumped to apcore 0.24.0 and apcore-toolkit 0.8.1.** `Cargo.toml`
  dependencies raised from `apcore = "0.22"` / `apcore-toolkit = "=0.8.0"` to
  `apcore = "0.24"` / `apcore-toolkit = "=0.8.1"`, tracking the aligned apcore 0.24.0
  and apcore-toolkit 0.8.1 releases (both resolve from crates.io). **No source
  changes** — the full test suite passes unchanged.

  The apcore 0.22.0 → 0.24.0 delta does not touch any surface the CLI consumes:
  - **Schema type coercion now default-on; `SchemaValidator` returns the coerced
    value** — the CLI does not use apcore's `SchemaValidator`. It implements its own
    JSON-Schema → clap translator (`schema_parser.rs`) and validates via the
    `jsonschema` crate directly (`cli.rs`), so the coercion change has no effect.
  - **Per-instance `ToggleState` isolation (#71)** — the CLI never constructs
    `ToggleState`/`APCore` nor calls `is_module_disabled()`; toggling is delegated to
    `system.*` modules via `Executor::call()`.
  - **Error `details` snake_case alignment (A-D-019)** — the CLI reads only
    `err.code` (for exit-code mapping); it never serializes apcore error `details`.
  - **`Registry::list()/get_definition()`**, **`Executor::call()/set_approval_handler()`**,
    `Config::default()`, the `ApprovalHandler` trait, and `ModuleAnnotations` are
    unchanged across the delta.
  - Out of scope and unused by the CLI: registry-event delivery/DLQ (A-D-013),
    middleware `on_error` (A-D-010/012/015), `APCore.on()/events()` bus (D1-011),
    array redaction (A-D-003), `Config` env coercion (A-D-007/009),
    `CircuitBreakerMiddleware`, `Context::create()`.

## [0.10.0] - 2026-05-18

### Changed — BREAKING (feature surface)

- **Removed `toolkit` Cargo feature flag — apcore-toolkit is now unconditionally required (resolves 6.2; lands ADR-07).** `apcore-toolkit = "=0.7.0"` was already declared as a hard runtime dependency in `Cargo.toml`, but the code base wrapped every toolkit-delegating path in `#[cfg(feature = "toolkit")]` and provided silent-downgrade fallbacks under `#[cfg(not(feature = "toolkit"))]`. This created the same "fake optional" self-contradiction the PY / TS 0.10.0 release fixed: required at the manifest level, soft-degraded at the code level. The sweep landed in this release:
  - Deleted 10 `#[cfg(feature = "toolkit")]` gates across `src/output.rs` (descriptor adapter, markdown/skill arms in `format_module_list` / `format_module_detail`, and 5 test helpers / tests) and `src/main.rs` (toolkit-integration block).
  - Deleted 3 `#[cfg(not(feature = "toolkit"))]` fallback branches that silently degraded `--format markdown` / `--format skill` to JSON with a `tracing::warn!`.
  - Removed the now-dead `TOOLKIT_MISSING_HINT` const.
  - Removed `toolkit = []` and `default = ["toolkit"]` from `Cargo.toml` `[features]`. Only `test-support` remains.
- **Migration for downstream crates** that depended on `apcore-cli` with `default-features = false`: explicitly opting out of the `toolkit` feature was previously a way to compile without the toolkit code paths (at the cost of silent format downgrade); that option is gone. apcore-toolkit will always be linked. If you genuinely cannot tolerate the toolkit dependency, pin to `apcore-cli = "0.9"` and stay there until you can adopt the unified surface.

## [0.9.0] - 2026-05-13

### Added

- **`tests/conformance_snake_case_kwargs.rs`** — runs the cross-language Algorithm C-SNAKE fixture (`apcore-cli/conformance/fixtures/snake-case-kwargs/cases.json`) against `schema_to_clap_args` + `reconcile_bool_pairs`, mirroring `extract_cli_kwargs`'s extraction path. Five cases verify that schema property names with underscores (`has_solution`, `sort_by`, `sort_order`) survive the round trip from clap parse to the kwargs dict. No source change required — clap's `Arg::new(prop_name)` keeps the snake_case id as the access key; the Rust SDK is a parity reference for the parallel TypeScript fix. Surfaced as part of the cross-SDK regression coverage gap audit.
### Fixed (2026-05-13 — cross-SDK audit D10/D11/D1)

- **Sandbox output-cap raises wrong error class** (D11-007) — byte-cap overflow now returns `ModuleExecutionError::OutputSizeExceeded { module_id, limit_bytes, overflow_stream }` instead of `OutputParseFailed`. Display message uses MiB units and names the overflowing stream (`stdout`/`stderr`/`stdout+stderr`), matching Python and TypeScript. `src/security/sandbox.rs:374`.
- **`exec --dry-run` emits Rust-only "Pipeline preview" stderr block** (D11-011) — the preview was not declared in the spec and had no Python/TS equivalent. Removed for cross-SDK parity; `--trace` now uniformly routes through `executor.call_with_trace` across all three SDKs.
- **CLI brand string inconsistency in error messages** (D11-006) — `src/security/config_encryptor.rs:56` `DecryptFailed` error text changed from `apcore-cli config set` to canonical `apcli config set`, matching `src/security/auth.rs` which already used `apcli`.
- **Unused `schemars` dev-dependency** (D6 re-audit) — `schemars = "0.8"` in `[dev-dependencies]` had zero usage (`use schemars`, `#[derive(JsonSchema)]`). Removed.
- **Stale `CLAUDE.md` `Sandbox::execute` arity claim** (D10 re-audit) — the "Current Conventions" bullet claimed Rust used a 2-parameter signature with executor bound at construction time; actual source has been 3-parameter since v0.7. Updated to reflect the real 3-parameter form.

### Added

- **`set_all_options_help` cross-SDK parity note** (D1-W1) — `src/cli.rs:104` doc-comment now documents that Rust intentionally ships without the deprecated `set_verbose_help` alias (post-rename, no pre-v0.9 callers). Python/TS keep the alias for one MINOR deprecation cycle.
- **`ConfigResolver::resolve` language-idiom note** (D10-W1) — `src/config.rs:113` doc-comment documents that Rust narrows the return to `Option<String>` (serde_yaml_ng string-coercion) while Python returns `Any` and TypeScript returns `unknown`. Embedders needing typed YAML access are pointed at a v0.10 typed-resolver follow-up.
- **`AuthProvider` encryptor-fallback language-idiom note** (D11-005) — `src/security/auth.rs` now documents that Rust's two-tier encryptor chain (explicit arg → fresh instance) differs from Python/TS's three-tier chain (explicit arg → `config.encryptor` peer attribute → fresh instance). The peer-attribute tier requires a `ConfigResolver` field addition tracked as a v0.10 follow-up.

### Fixed

- **CSV `--format csv` heterogeneous-keys data loss** — `format_exec_result` previously derived headers from `arr[0].as_object().keys()` only, silently dropping fields that first appeared in later rows. Now delegates to `apcore_toolkit::format_csv()` which uses the **union of keys across all rows** in insertion-order. `src/output.rs:537-566`.
- **CSV line terminator** — now `\r\n` per RFC 4180 (was `\n`). Existing test expectations updated; old `\n`-based assertions replaced with CRLF assertions in `tests/test_output.rs`.

### Changed

- **User-visible help/man/completion text no longer leaks the `apcore` framework name** to end users of downstream CLIs built on apcore-cli. Affected strings: `--extensions-dir` option help (`Path to apcore extensions directory.` → `Path to extensions directory.`, `src/main.rs:367`), `exec` subcommand description (`Execute an apcore module` → `Execute a module`, `src/shell.rs:62`, `src/cli.rs:344`, plus the `shell.rs:1102` test fixture and the `tests/test_shell.rs:13-14` integration-test fixture), and man-page `ENVIRONMENT` text in `src/shell.rs:640, 653, 658` (`Path to the apcore extensions directory` → `Path to the extensions directory`, `Global apcore logging verbosity` → `Global logging verbosity`, `API key for authenticating with the apcore registry` → `API key for authenticating with the registry`). README's `--verbose` row updated to match. The `test_generate_man_page_name_uses_description` assertion updated to the new "about" text. Logger fields, source comments, doc comments, and environment-variable identifiers (`APCORE_*`) are unchanged — only descriptive copy that appears in `--help`, shell completion, and `man` output. Cross-SDK parity with Python 0.8.1 and TypeScript 0.8.2.

### Changed (breaking CLI surface)

- **Global `--verbose` flag renamed to `--all-options`** — The help-display flag is now `--all-options`; use `apcore-cli module --help --all-options` to reveal hidden built-in options. `verbose` is removed from the reserved schema property names set — module schemas may now freely define `verbose: boolean` for runtime output control. Public API: `set_verbose_help` / `is_verbose_help` renamed to `set_all_options_help` / `is_all_options_help`; statics renamed accordingly. Tracked in [apcore-cli#21](https://github.com/aiperceivable/apcore-cli/issues/21).

### Changed (breaking feature-flag semantics)

- **`apcore-toolkit` promoted from optional Cargo feature to REQUIRED runtime dependency** (`>=0.7.0`). The `toolkit` feature flag is retained in `default` features for backward compatibility — existing `#[cfg(feature = "toolkit")]` gates continue to work — but consumers using `default-features = false` must explicitly enable `features = ["toolkit"]` to compile. Reqired because csv/yaml/jsonl now route through the toolkit's reference implementation.
- **`serde_json::Map` iteration order** — transitively switched to insertion-order via the toolkit's `preserve_order` feature. Test assertions that relied on alphabetical iteration (`tests/test_output.rs::test_csv_plain_value_passthrough`) updated to expect insertion-order.

### Removed

- `csv_scalar_string` and `csv_field` private helpers — replaced by `apcore_toolkit::format_csv()` and the toolkit's RFC 4180 internals.

### Why

See ADR-09 in `apcore-cli/docs/tech-design.md` for the byte-equivalent toolkit-delegated tier rationale.

## [0.8.0] - 2026-05-08

### Security

- **D10-001 (critical) — `AuthProvider::authenticate_request` rejects trailing CR/LF in the API key** (`src/security/auth.rs`). Previous behaviour stripped trailing `\r`/`\n` before the malformed-key check, allowing a key ending in `"\n"` to silently produce `Authorization: Bearer <stripped>` and exposing the SDK to header-injection vectors. Python and TypeScript both reject any `\r` or `\n` at any position; Rust now matches. The regression test `test_authenticate_request_strips_trailing_crlf` was asserting the wrong behaviour and has been renamed to `test_authenticate_request_rejects_trailing_crlf` with the assertion inverted.
- **D10-truncated #1 — `ConfigEncryptor::retrieve` surfaces a user-actionable decryption error** (`src/security/config_encryptor.rs`). All `retrieve()`-time decryption failures (b64 decode, v1 AES, v2 AES) now route through a new `ConfigEncryptorError::DecryptFailed { key }` variant with the spec'd message `"Failed to decrypt configuration value '{key}'. Re-configure with 'apcore-cli config set {key}'."` Previously, the most common decryption failure modes leaked the internal `AuthTagMismatch` message and dropped both the originating config key and the remediation guidance. `AuthTagMismatch` is preserved as the internal-helper variant returned from `_aes_decrypt_v1`/`_aes_decrypt_v2`. Cross-SDK parity with Python (`config_encryptor.py:62-64,70-72`) and TypeScript (`config-encryptor.ts:136-149`).
- **D11-001 — Built-in-group rename surface** (`src/builtin_group.rs`, `src/cli.rs`, `src/main.rs`, `src/lib.rs`). Restores FE-13 P0 parity with Python `ApcliGroup.name` and TypeScript `ApcliGroup#name`. New `pub fn ApcliGroup::name(&self) -> &str` accessor backed by a `name: String` field that defaults to `"apcli"` and is validated against `NAME_REGEX = ^[a-z][a-z0-9_-]*$`. New factory variants `from_cli_config_with_name` / `from_yaml_with_name` / `try_from_yaml_with_name` accept `name: Option<String>`; the original 2-arg factories delegate with `None` for backward compatibility. New `validate_builtin_group_name` helper and `ApcliGroupError::InvalidName` variant. New module-level `effective_reserved_group_names`, `is_reserved_group_name`, and `pub fn set_reserved_group_names(...)` (mirrors TypeScript `setReservedGroupNames`); `cli.rs::build_module_command_with_limit` consults the live set so a renamed built-in group is honoured at collision-check time. Binary entry-point seeds the live set from `apcli_cfg.name()` and threads the resolved name through the `clap::Command::new(...)` builder.
- **D11-W3 — Sandbox canonicalises inherited `APCORE_EXTENSIONS_ROOT`** (`src/security/sandbox.rs:248`). The child env now carries an absolute, symlink-resolved path so sandboxed processes cannot escape via a relative or symlink-bait extensions root.
- **D11-002 — `sorted_json` recurses into nested objects and arrays** for hash canonicalisation (`src/security/audit.rs:20`). Previously only top-level keys were sorted, so audit-log input hashes diverged for inputs with nested structures. Aligns Rust with the Python and TypeScript canonicalisation contract.

### Added

- **D11-001 — `pub fn set_reserved_group_names(names: &[String])`** module-level setter on `builtin_group` (mirrors TypeScript `setReservedGroupNames`) plus the `ApcliGroup::name()` accessor and `from_cli_config_with_name` / `from_yaml_with_name` / `try_from_yaml_with_name` factory variants. See Security entry above for the full surface.
- **D1-004 — `Sandbox::with_extensions_root(...)` and `Sandbox::with_max_output_bytes(...)` builder methods** (`src/security/sandbox.rs`). Cross-SDK parity with Python `Sandbox.with_extensions_root` / `with_max_output_bytes` (`apcore-cli-python/src/apcore_cli/security/sandbox.py:85,95`). `extensions_root` overrides any inherited `APCORE_EXTENSIONS_ROOT` env var with a canonicalised absolute path; `max_output_bytes` replaces the `SANDBOX_OUTPUT_SIZE_LIMIT_BYTES` constant as the per-instance output cap. Both fields are wired through to `_sandboxed_execute`. 5 new unit tests + 4 new integration tests cover field defaults, single-setter behaviour, fluent chaining, and the disabled-path passthrough invariant.
- **`CliError::SchemaParserFailure { module_id, source }` variant** in `src/cli.rs` — wraps `SchemaParserError::ReservedPropertyName` and `::FlagCollision` so both route to `EXIT_SCHEMA_CIRCULAR_REF` (48) via `CliError::exit_code()`. Previously these errors were re-wrapped as `CliError::InvalidModuleId` and exited with code 2, breaking cross-SDK exit-code parity with Python `sys.exit(48)` and TypeScript `process.exit(EXIT_CODES.SCHEMA_CIRCULAR_REF)`. Audit D11-NEW-005 (see Fixed).
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
- **D5-002 — Dedicated unit tests for `builtin_group` and `display_helpers`** (`tests/test_builtin_group.rs`, `tests/test_display_helpers.rs`). 10 tests cover `APCLI_SUBCOMMAND_NAMES` / `RESERVED_GROUP_NAMES` constants, all four `from_cli_config` modes, both auto-detect branches, and both `try_from_yaml` validation errors. 6 tests cover display-block extraction, alias precedence chain, and tag fallback chain.
- **D11-NEW-001 / D11-NEW-003 — `ref_resolver` preserves parent sibling `required` in `anyOf`/`oneOf`** (`src/ref_resolver.rs`). `resolve_node` now extracts `sibling_required` from the parent before the branch loop and merges it sibling-first deduplicated with the branch intersection at the end; `merged.properties` is also seeded from the parent (parity with the existing `allOf` branch handling). 3 new regression tests cover `anyOf`, `oneOf`, and dedup overlap. Matches Python `ref_resolver.py:100-118`.
- **`Documented parity gap for the built-in-group rename feature`** in `src/lib.rs` (now superseded by D11-001 above — kept here for the comment block listing the implementation requirements that have since landed).
- **D1-006 — Documented `allowed_prefixes` parity gap** in `src/lib.rs`. TypeScript `createCli` gained `allowedPrefixes` (commit `0f2e08a`); Rust cannot mirror it until the high-level embedding factory (removed in v0.7.0 D9-001/002) returns. The lib-level cross-SDK parity note now records that TypeScript is no longer missing it and Rust is the sole gap.

### Changed

- **D6-W1 — `serde_yaml` replaced with `serde_yaml_ng = "0.10"`** (`Cargo.toml:29`). Upstream `serde_yaml` was deprecated; `serde_yaml_ng` is the maintained drop-in replacement. No API surface change.
- **D6-003 — `apcore` pin policy relaxed** from `=0.21.0` to `0.21` (minor floor), aligning with `apcore-cli-python` (`>=0.21.0`) and `apcore-cli-typescript` (`>=0.21.0`).
- **Dependency bumps** — `nix 0.29 → 0.30.1`, `thiserror 1 → 2.0.18`, `comfy-table 6 → 7.2.2` (transitive: `crossterm 0.26 → 0.29`, `unicode-width 0.1 → 0.2`).
- **`Makefile` `coverage` target** now passes `--fail-under-lines 85` to `cargo llvm-cov`, matching the Python `pyproject.toml` `[tool.coverage.report] fail_under = 85` and the new TypeScript `vitest.config.ts` `thresholds.lines: 85`. Cross-SDK CI parity (audit D5-004).
- **`apcli list` and `apcli describe` `--format` value-parsers** expanded to
  `[table, json, csv, yaml, jsonl, markdown, skill]`. `describe` previously
  accepted only `[table, json]`. Unknown values exit with code 2 (clap
  rejection) as before. Issue
  [aiperceivable/apcore-cli#20](https://github.com/aiperceivable/apcore-cli/issues/20).
- **Dependency bump**: `apcore = "0.21"` (was `=0.19.0`) and the optional
  `apcore-toolkit = "=0.6.0"` (was `=0.5.0`). Aligns with upstream
  `apcore 0.21.0` (`Module::preview` / `PreflightResult::predicted_changes`)
  and `apcore-toolkit 0.6.0` (surface-aware formatters). No CLI-visible
  behavioural breaks.
- **D8-W1 — `Cargo.lock` is now tracked** in git. Per Cargo guidance, the lockfile must be committed for crates that ship a `[[bin]]` target to guarantee reproducible binary builds. The lockfile was previously gitignored.
- **D9-W5 — `register_completion_command` no longer takes `prog_name`** (`src/shell.rs:79`). The parameter was unused; signature now matches the TypeScript `registerCompletionCommand` contract.

### Fixed

- **D11-NEW-005 — `schema_to_clap_args` `Err(SchemaParserError::*)` was mapped to exit code 2**, not 48. The call site in `src/cli.rs:425` previously wrapped both `ReservedPropertyName` and `FlagCollision` as `CliError::InvalidModuleId`, which exits 2. Both are spec-defined exit-48 schema-validity errors per `apcore-cli/docs/features/schema-parser.md` Contract: `schema_to_click_options` Errors (cross-SDK parity with Python `sys.exit(48)` and TS `process.exit(EXIT_CODES.SCHEMA_CIRCULAR_REF)`). Fix routes through the new `CliError::SchemaParserFailure` variant.
- **D9-NEW-002 — `merge_allof` did not deduplicate `required` across branches**. The function concatenated each branch's `required` array via `.extend()`, producing duplicates when two branches independently required the same field name. Spec mandates first-seen-wins dedup (matching TypeScript `[...new Set(...)]` and Python's new explicit seen-set). Fix: replace `.extend()` with a `for item in req { if !merged_required.contains(item) { merged_required.push(...) } }` loop. Outer `obj.required` parent-vs-branches dedup at line 244-251 was already correct.
- **D10-002 — `resolve_refs` exit-code split** (`src/cli.rs:69`). `RefResolverError::Unresolvable` now exits `45` (`EXIT_SCHEMA_REF_UNRESOLVABLE`) while `RefResolverError::Circular` and `RefResolverError::MaxDepthExceeded` exit `48` (`EXIT_SCHEMA_CIRCULAR_REF`). Previously all three collapsed onto a single exit code, breaking cross-SDK parity with Python `sys.exit(45)` / `sys.exit(48)` and the TypeScript `EXIT_CODES.SCHEMA_REF_UNRESOLVABLE` / `SCHEMA_CIRCULAR_REF` split.
- **D10-W1 + D11-W5 — `schema_parser` flag-collision check probes `seen_flags` before inserting the synthetic `--no-X`** (`src/schema_parser.rs:280`), and the collision message now references the original boolean property name instead of the negated form. Cross-SDK message parity.
- **D10-truncated #3 — Clarified `CliApprovalHandler::check_approval` shadow** (`src/approval.rs`). Added a doc-comment disambiguation table covering both `check_approval` overloads (the inherent method that takes `&Value` and is an alias for `request_approval`, and the `apcore::ApprovalHandler` trait impl that takes `&str` and implements the spec's Phase B polling protocol returning `"rejected — CLI does not support async polling"`). The previous comment claimed the inherent method "matches the Python/TypeScript `check_approval` method name", which was misleading. Doc-only change.
- **D11-W1 — `ConfigEncryptor` username fallback chain extended to `USER → LOGNAME → USERNAME`** (`src/security/config_encryptor.rs:233`) for Windows parity with the Python and TypeScript SDKs.
- **D9-W3 — `register_discovery_commands` deleted; `cmd_list` demoted to `pub(crate)`** (`src/discovery.rs:313`). The wrapper had no remaining callers and exposed an internal helper that was never part of the spec'd surface.
- **D10-info-1 — `APCORE_CLI_APCLI` env value is now trimmed before lowercase normalisation** (`src/builtin_group.rs:633`). Spec invariant 2 (`apcore-cli/docs/features/builtin-group.md`) requires the env-var parser to be both case-insensitive and trim-on-read; values like `"  show  "` or `"\thide\n"` now resolve to `"all"`/`"none"` instead of hitting the warn-and-fallthrough branch. Pure-whitespace strings collapse to `"unset"` (parity with the empty-string short-circuit) rather than warning.
- **D11-010 — `AuditLogger` write-failure warnings are deduplicated**. Repeated IO failures against the same `AuditLogger` instance now emit `"Could not write audit log"` at most once; subsequent failures fall through to `trace` level. The dedup flag lives in `Arc<AtomicBool>` so clones share state, matching TypeScript `_writeFailureWarned` and Python `_write_failure_warned` (`src/security/audit.rs:227`).
- **D11-011 — `ExposureFilter` accepts `mode = "none"` silently** (`src/exposure.rs:59`). Python and TypeScript treat `"none"` as a legitimate user-supplied value (hides every module); Rust was warning `"Unknown ExposureFilter mode 'none'"` and clamping back to `"none"`. The end-state was identical, but the spurious warning broke log-noise parity. `"none"` is now in the `VALID_MODES` whitelist; truly unknown modes still warn-and-clamp (fail-closed).

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
- **D6-002 — `tokio-test = "0.4"` dev-dependency removed**. The crate had zero references across `src/`, `tests/`, and `examples/`; `#[tokio::test]` macros come from tokio's own `macros` feature.
- **D9-W3 — `register_discovery_commands` wrapper removed** from `src/discovery.rs`. See Fixed entry above.

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

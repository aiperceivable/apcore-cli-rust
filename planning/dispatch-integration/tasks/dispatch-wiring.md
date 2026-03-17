# Task: Wire external + exec to dispatch_module

## Goal

Connect both the `exec` subcommand match arm and the external (catch-all) subcommand match arm in `main.rs` to `cli::dispatch_module`, replacing the current "not yet implemented" stub.

## Files Involved

- `src/main.rs` -- subcommand dispatch `match` block
- `src/cli.rs` -- `dispatch_module` signature (read-only reference, no changes)

## Steps (TDD)

1. **Write integration test** (or verify manually): invoking `apcore-cli exec <invalid_module>` should exit 2 (invalid module ID) or exit 44 (not found), not the old "not yet implemented" message.

2. **Wire `exec` arm** in `main.rs` dispatch match:
   ```rust
   Some(("exec", sub_m)) => {
       let module_id = sub_m.get_one::<String>("module_id")
           .expect("module_id is required");
       let executor: Arc<dyn apcore_cli::ModuleExecutor> =
           Arc::new(apcore_cli::cli::ApCoreExecutorAdapter(apcore::Executor::new()));
       let apcore_executor = apcore::Executor::new();
       apcore_cli::cli::dispatch_module(
           module_id, sub_m, &registry_provider, &executor, &apcore_executor
       ).await;
   }
   ```

3. **Wire external (catch-all) arm** -- replace the eprintln + exit stub:
   ```rust
   Some((external, sub_m)) => {
       let executor: Arc<dyn apcore_cli::ModuleExecutor> =
           Arc::new(apcore_cli::cli::ApCoreExecutorAdapter(apcore::Executor::new()));
       let apcore_executor = apcore::Executor::new();
       apcore_cli::cli::dispatch_module(
           external, sub_m, &registry_provider, &executor, &apcore_executor
       ).await;
   }
   ```

4. **Ensure `registry_provider` type is `Arc<dyn RegistryProvider>`** -- it already is in the current code, but verify the import path is consistent between the `exec` and external arms.

5. **Run tests**: `cargo test`.

6. **Run clippy**: `cargo clippy -- -D warnings`.

## Acceptance Criteria

- The "Dynamic module dispatch not yet implemented" message is completely removed.
- `apcore-cli exec math.add` routes to `dispatch_module`.
- `apcore-cli math.add` (external subcommand) routes to `dispatch_module`.
- `dispatch_module` controls exit codes (44 for not found, 2 for invalid, etc.).
- All existing tests pass.

## Dependencies

- `exec-subcommand` -- the `exec` subcommand must be registered before it can be matched.
- `validate-extensions-result` -- if `validate_extensions_dir` still calls `process::exit`, the dispatch path may not be reachable in test scenarios. This dependency is soft: wiring can proceed first, but the validate refactor makes testing easier.

## Estimated Time

~60 minutes

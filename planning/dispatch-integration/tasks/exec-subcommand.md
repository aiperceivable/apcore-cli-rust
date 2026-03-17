# Task: Register exec subcommand

## Goal

Add an `exec` subcommand to the clap command tree so that `apcore-cli exec <MODULE_ID> [flags]` is a recognized command. Currently `exec` appears in `KNOWN_BUILTINS` and man page generation, but is not registered as a clap subcommand.

## Files Involved

- `src/main.rs` -- `build_cli_command` function, add `exec` subcommand registration
- `src/cli.rs` -- may need a public `exec_command()` builder function (or inline in main.rs)
- `src/shell.rs` -- already lists `exec` in `KNOWN_BUILTINS`; no changes needed

## Steps (TDD)

1. **Write test** in `src/main.rs::tests`: verify that `build_cli_command(None, None, false)` has an `exec` subcommand with:
   - Required positional `MODULE_ID` argument
   - Optional `--input` / `--stdin` flag for piped input
   - Optional `--yes` flag for auto-approval
   - Optional `--large-input` flag
   - Optional `--format` flag

2. **Implement `exec_command()` builder** in `src/cli.rs` (public):
   ```rust
   pub fn exec_command() -> clap::Command {
       Command::new("exec")
           .about("Execute an apcore module")
           .arg(Arg::new("module_id").required(true).value_name("MODULE_ID"))
           .arg(Arg::new("input").long("input").value_name("SOURCE"))
           .arg(Arg::new("yes").long("yes").short('y').action(ArgAction::SetTrue))
           .arg(Arg::new("large-input").long("large-input").action(ArgAction::SetTrue))
           .arg(Arg::new("format").long("format").value_parser(["table", "json"]))
   }
   ```

3. **Register in `build_cli_command`** in `main.rs`:
   ```rust
   cmd = cmd.subcommand(apcore_cli::cli::exec_command());
   ```

4. **Run tests**: `cargo test` -- confirm the new test passes and no existing tests break.

5. **Run clippy**: `cargo clippy -- -D warnings`.

## Acceptance Criteria

- `apcore-cli exec --help` prints usage with MODULE_ID positional and expected flags.
- `build_cli_command` returns a command tree that includes `exec`.
- Man page generation for `exec` still works (it already stubs via `KNOWN_BUILTINS`, now it picks up the real subcommand).
- All existing tests pass.

## Dependencies

None.

## Estimated Time

~45 minutes

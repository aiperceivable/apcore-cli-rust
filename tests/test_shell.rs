// apcore-cli — Integration tests for shell completion and man page commands.
// Protocol spec: FE-10 (FR-SHELL-001, FR-SHELL-002)

mod common;

use apcore_cli::shell::{register_shell_commands, ShellError};
use clap::Command;
use clap_complete::Shell;

fn make_root_cmd() -> Command {
    Command::new("apcore-cli")
        .version("0.2.0")
        .about("Command-line interface for apcore modules")
        .subcommand(Command::new("exec").about("Execute an apcore module"))
        .subcommand(Command::new("list").about("List available modules"))
        .subcommand(Command::new("describe").about("Show module metadata and schema"))
}

#[test]
fn test_register_shell_commands_adds_completion() {
    let root = register_shell_commands(make_root_cmd(), "apcore-cli");
    let names: Vec<_> = root.get_subcommands().map(|c| c.get_name()).collect();
    assert!(
        names.contains(&"completion"),
        "root must have 'completion' subcommand, got: {names:?}"
    );
}

#[test]
fn test_register_shell_commands_adds_man() {
    let root = register_shell_commands(make_root_cmd(), "apcore-cli");
    let names: Vec<_> = root.get_subcommands().map(|c| c.get_name()).collect();
    assert!(
        names.contains(&"man"),
        "root must have 'man' subcommand, got: {names:?}"
    );
}

#[test]
fn test_completion_bash_outputs_nonempty() {
    let mut cmd = make_root_cmd();
    let output = apcore_cli::shell::cmd_completion(Shell::Bash, "apcore-cli", &mut cmd);
    assert!(!output.is_empty(), "bash completion must not be empty");
}

#[test]
fn test_completion_zsh_outputs_nonempty() {
    let mut cmd = make_root_cmd();
    let output = apcore_cli::shell::cmd_completion(Shell::Zsh, "apcore-cli", &mut cmd);
    assert!(!output.is_empty(), "zsh completion must not be empty");
}

#[test]
fn test_completion_fish_outputs_nonempty() {
    let mut cmd = make_root_cmd();
    let output = apcore_cli::shell::cmd_completion(Shell::Fish, "apcore-cli", &mut cmd);
    assert!(!output.is_empty(), "fish completion must not be empty");
}

#[test]
fn test_completion_invalid_shell_rejected_at_parse() {
    // clap rejects unknown shell values at parse time; verify the arg definition
    // uses a value_parser that does not accept arbitrary strings.
    use apcore_cli::shell::completion_command;
    let cmd = completion_command();
    let shell_arg = cmd.get_arguments().find(|a| a.get_id() == "shell");
    assert!(
        shell_arg.is_some(),
        "completion_command must have a 'shell' argument"
    );
    // Verify parse-time rejection by attempting to parse an invalid value.
    let result = cmd
        .clone()
        .try_get_matches_from(["completion", "invalid-shell"]);
    assert!(
        result.is_err(),
        "completion with invalid shell must be rejected by clap"
    );
}

#[test]
fn test_man_command_outputs_nonempty_for_known_builtin() {
    use apcore_cli::shell::cmd_man;
    let root = make_root_cmd();
    let result = cmd_man("list", &root, "apcore-cli", "0.2.0");
    assert!(result.is_ok(), "man for known builtin 'list' must succeed");
    let page = result.unwrap();
    assert!(!page.is_empty(), "man page must not be empty");
    assert!(page.contains(".TH"), "man page must contain .TH");
}

#[test]
fn test_man_command_outputs_nonempty_for_exec() {
    use apcore_cli::shell::cmd_man;
    let root = make_root_cmd();
    let result = cmd_man("exec", &root, "apcore-cli", "0.2.0");
    assert!(result.is_ok(), "man for 'exec' must succeed");
    let page = result.unwrap();
    assert!(
        page.contains(".SH EXIT CODES"),
        "man page must have EXIT CODES section"
    );
}

#[test]
fn test_man_command_unknown_returns_error() {
    use apcore_cli::shell::cmd_man;
    let root = make_root_cmd();
    let result = cmd_man("bogus-command", &root, "apcore-cli", "0.2.0");
    assert!(result.is_err());
    match result.unwrap_err() {
        ShellError::UnknownCommand(name) => assert_eq!(name, "bogus-command"),
    }
}

#[test]
#[cfg(unix)]
fn test_completion_bash_valid_syntax() {
    // Validate bash completion script with `bash -n`.
    use std::io::Write;
    let mut cmd = make_root_cmd();
    let script = apcore_cli::shell::cmd_completion(Shell::Bash, "apcore-cli", &mut cmd);
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(script.as_bytes()).unwrap();
    let status = std::process::Command::new("bash")
        .arg("-n")
        .arg(tmpfile.path())
        .status();
    match status {
        Ok(s) => assert!(s.success(), "bash -n failed on generated completion script"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // bash not installed — skip silently
        }
        Err(e) => panic!("failed to run bash: {e}"),
    }
}

// apcore-cli — Integration tests for shell completion and man page commands.
// Protocol spec: FE-10 (FR-SHELL-001, FR-SHELL-002)

mod common;

use apcore_cli::shell::{register_completion_command, register_man_command, ShellError};
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

// Embedders compose the registrars directly (see lib.rs note re: D9-003 removal
// of the register_shell_commands wrapper). These tests verify the same
// post-composition shape that the wrapper produced.

#[test]
fn test_compose_registrars_adds_completion() {
    let root = register_man_command(register_completion_command(make_root_cmd(), "apcore-cli"));
    let names: Vec<_> = root.get_subcommands().map(|c| c.get_name()).collect();
    assert!(
        names.contains(&"completion"),
        "root must have 'completion' subcommand, got: {names:?}"
    );
}

#[test]
fn test_compose_registrars_adds_man() {
    let root = register_man_command(register_completion_command(make_root_cmd(), "apcore-cli"));
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

// --- build_program_man_page ---

#[test]
fn build_program_man_page_generates_roff() {
    let cmd = clap::Command::new("test-cli").about("Test CLI").subcommand(
        clap::Command::new("hello")
            .about("Say hello")
            .arg(clap::Arg::new("name").long("name").help("Your name")),
    );
    let roff = apcore_cli::shell::build_program_man_page(&cmd, "test-cli", "1.0.0", None, None);
    assert!(roff.contains(".TH \"TEST-CLI\""));
    assert!(roff.contains(".SH COMMANDS"));
    assert!(roff.contains("hello"));
    assert!(roff.contains("\\-\\-name"));
}

#[test]
fn build_program_man_page_includes_nested() {
    let cmd = clap::Command::new("mycli").subcommand(
        clap::Command::new("grp").subcommand(
            clap::Command::new("sub")
                .about("A sub")
                .arg(clap::Arg::new("flag").long("flag").help("A flag")),
        ),
    );
    let roff = apcore_cli::shell::build_program_man_page(&cmd, "mycli", "1.0.0", None, None);
    assert!(roff.contains("mycli grp sub"));
}

#[test]
fn build_program_man_page_uses_explicit_description() {
    let cmd = clap::Command::new("mycli").about("Default desc");
    let roff = apcore_cli::shell::build_program_man_page(
        &cmd,
        "mycli",
        "1.0.0",
        Some("Custom description"),
        None,
    );
    assert!(roff.contains("Custom description"));
}

#[test]
fn build_program_man_page_includes_environment() {
    let cmd = clap::Command::new("mycli");
    let roff = apcore_cli::shell::build_program_man_page(&cmd, "mycli", "1.0.0", None, None);
    assert!(roff.contains(".SH ENVIRONMENT"));
    assert!(roff.contains("APCORE_EXTENSIONS_ROOT"));
}

#[test]
fn build_program_man_page_includes_exit_codes() {
    let cmd = clap::Command::new("mycli");
    let roff = apcore_cli::shell::build_program_man_page(&cmd, "mycli", "1.0.0", None, None);
    assert!(roff.contains(".SH EXIT CODES"));
    assert!(roff.contains("\\fB0\\fR"));
    assert!(roff.contains("\\fB130\\fR"));
}

// --- has_man_flag ---

#[test]
fn has_man_flag_detects_flag() {
    let args: Vec<String> = vec!["--help".into(), "--man".into()];
    assert!(apcore_cli::shell::has_man_flag(&args));
}

#[test]
fn has_man_flag_returns_false_when_absent() {
    let args: Vec<String> = vec!["--help".into()];
    assert!(!apcore_cli::shell::has_man_flag(&args));
}

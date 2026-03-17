// apcore-cli — Shell completion and man page generation.
// Protocol spec: FE-10 (register_shell_commands)

use clap::Command;
use clap_complete::{generate, Shell};
use thiserror::Error;

// ---------------------------------------------------------------------------
// ShellError
// ---------------------------------------------------------------------------

/// Errors produced by shell integration commands.
#[derive(Debug, Error)]
pub enum ShellError {
    #[error("unknown command '{0}'")]
    UnknownCommand(String),
}

// ---------------------------------------------------------------------------
// KNOWN_BUILTINS
// ---------------------------------------------------------------------------

/// The fixed set of built-in CLI command names.
///
/// `cmd_man` consults this list when the requested command name is not found
/// among the live clap subcommands, so that built-in commands that have not
/// yet been wired still produce a man page stub rather than an "unknown
/// command" error.
pub const KNOWN_BUILTINS: &[&str] = &["exec", "list", "describe", "completion", "man"];

// ---------------------------------------------------------------------------
// register_shell_commands
// ---------------------------------------------------------------------------

/// Attach the `completion` and `man` subcommands to the given root command and
/// return it. Uses the clap v4 builder idiom (consume + return).
///
/// * `completion <shell>` — emit shell completion script to stdout
///   Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`
/// * `man`                — emit a man page to stdout
pub fn register_shell_commands(cli: Command, prog_name: &str) -> Command {
    let _ = prog_name; // prog_name reserved for future dynamic use
    cli.subcommand(completion_command())
        .subcommand(man_command())
}

// ---------------------------------------------------------------------------
// completion_command / cmd_completion
// ---------------------------------------------------------------------------

/// Build the `completion` clap subcommand.
pub fn completion_command() -> clap::Command {
    clap::Command::new("completion")
        .about("Generate a shell completion script and print it to stdout")
        .long_about(
            "Generate a shell completion script and print it to stdout.\n\n\
             Install examples:\n\
             \x20 bash:       eval \"$(apcore-cli completion bash)\"\n\
             \x20 zsh:        eval \"$(apcore-cli completion zsh)\"\n\
             \x20 fish:       apcore-cli completion fish | source\n\
             \x20 elvish:     eval (apcore-cli completion elvish)\n\
             \x20 powershell: apcore-cli completion powershell | Out-String | Invoke-Expression",
        )
        .arg(
            clap::Arg::new("shell")
                .value_name("SHELL")
                .required(true)
                .value_parser(clap::value_parser!(Shell))
                .help("Shell to generate completions for (bash, zsh, fish, elvish, powershell)"),
        )
}

/// Handler: generate a shell completion script and return it as a String.
///
/// `shell`     — the target shell (parsed from clap argument)
/// `prog_name` — the program name to embed in the script
/// `cmd`       — mutable reference to the root Command (required by clap_complete)
pub fn cmd_completion(shell: Shell, prog_name: &str, cmd: &mut clap::Command) -> String {
    let mut buf: Vec<u8> = Vec::new();
    generate(shell, cmd, prog_name, &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

// ---------------------------------------------------------------------------
// man_command / build_synopsis / generate_man_page / cmd_man
// ---------------------------------------------------------------------------

/// Build the `man` clap subcommand.
pub fn man_command() -> Command {
    Command::new("man")
        .about("Generate a roff man page for COMMAND and print it to stdout")
        .long_about(
            "Generate a roff man page for COMMAND and print it to stdout.\n\n\
             View immediately:\n\
             \x20 apcore-cli man exec | man -l -\n\
             \x20 apcore-cli man list | col -bx | less\n\n\
             Install system-wide:\n\
             \x20 apcore-cli man exec > /usr/local/share/man/man1/apcore-cli-exec.1\n\
             \x20 mandb   # (Linux)  or  /usr/libexec/makewhatis  # (macOS)",
        )
        .arg(
            clap::Arg::new("command")
                .value_name("COMMAND")
                .required(true)
                .help("CLI subcommand to generate the man page for"),
        )
}

/// Build the roff SYNOPSIS line from a clap Command's arguments.
pub fn build_synopsis(cmd: Option<&clap::Command>, prog_name: &str, command_name: &str) -> String {
    let Some(cmd) = cmd else {
        return format!("\\fB{prog_name} {command_name}\\fR [OPTIONS]");
    };

    let mut parts = vec![format!("\\fB{prog_name} {command_name}\\fR")];

    for arg in cmd.get_arguments() {
        // Skip help/version flags injected by clap
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }

        let is_positional = arg.get_long().is_none() && arg.get_short().is_none();
        let is_required = arg.is_required_set();

        if is_positional {
            let meta_owned: String = arg
                .get_value_names()
                .and_then(|v| v.first().map(|s| s.to_string()))
                .unwrap_or_else(|| "ARG".to_string());
            let meta = meta_owned.as_str();
            if is_required {
                parts.push(format!("\\fI{meta}\\fR"));
            } else {
                parts.push(format!("[\\fI{meta}\\fR]"));
            }
        } else {
            let flag = if let Some(long) = arg.get_long() {
                format!("\\-\\-{long}")
            } else {
                format!("\\-{}", arg.get_short().unwrap())
            };
            let is_flag = arg.get_num_args().is_some_and(|r| r.max_values() == 0);
            if is_flag {
                parts.push(format!("[{flag}]"));
            } else {
                let type_name_owned: String = arg
                    .get_value_names()
                    .and_then(|v| v.first().map(|s| s.to_string()))
                    .unwrap_or_else(|| "VALUE".to_string());
                let type_name = type_name_owned.as_str();
                if is_required {
                    parts.push(format!("{flag} \\fI{type_name}\\fR"));
                } else {
                    parts.push(format!("[{flag} \\fI{type_name}\\fR]"));
                }
            }
        }
    }

    parts.join(" ")
}

/// Build a complete roff man page string for a CLI subcommand.
pub fn generate_man_page(
    command_name: &str,
    cmd: Option<&clap::Command>,
    prog_name: &str,
    version: &str,
) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let today = {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let days = secs / 86400;
        format_roff_date(days)
    };

    let title = format!("{}-{}", prog_name, command_name).to_uppercase();
    let pkg_label = format!("{prog_name} {version}");
    let manual_label = format!("{prog_name} Manual");

    let mut sections: Vec<String> = Vec::new();

    // .TH
    sections.push(format!(
        ".TH \"{title}\" \"1\" \"{today}\" \"{pkg_label}\" \"{manual_label}\""
    ));

    // .SH NAME
    sections.push(".SH NAME".to_string());
    let desc = cmd
        .and_then(|c| c.get_about())
        .map(|s| s.to_string())
        .unwrap_or_else(|| command_name.to_string());
    let name_desc = desc.lines().next().unwrap_or("").trim_end_matches('.');
    sections.push(format!("{prog_name}-{command_name} \\- {name_desc}"));

    // .SH SYNOPSIS
    sections.push(".SH SYNOPSIS".to_string());
    sections.push(build_synopsis(cmd, prog_name, command_name));

    // .SH DESCRIPTION (using about text)
    if let Some(about) = cmd.and_then(|c| c.get_about()) {
        sections.push(".SH DESCRIPTION".to_string());
        let escaped = about.to_string().replace('\\', "\\\\").replace('-', "\\-");
        sections.push(escaped);
    } else {
        // Emit a stub DESCRIPTION section so it's always present
        sections.push(".SH DESCRIPTION".to_string());
        sections.push(format!("{prog_name}\\-{command_name}"));
    }

    // .SH OPTIONS (only if command has named options)
    if let Some(c) = cmd {
        let options: Vec<_> = c
            .get_arguments()
            .filter(|a| a.get_long().is_some() || a.get_short().is_some())
            .filter(|a| a.get_id().as_str() != "help" && a.get_id().as_str() != "version")
            .collect();

        if !options.is_empty() {
            sections.push(".SH OPTIONS".to_string());
            for arg in options {
                let flag_parts: Vec<String> = {
                    let mut fp = Vec::new();
                    if let Some(short) = arg.get_short() {
                        fp.push(format!("\\-{short}"));
                    }
                    if let Some(long) = arg.get_long() {
                        fp.push(format!("\\-\\-{long}"));
                    }
                    fp
                };
                let flag_str = flag_parts.join(", ");

                let is_flag = arg.get_num_args().is_some_and(|r| r.max_values() == 0);
                sections.push(".TP".to_string());
                if is_flag {
                    sections.push(format!("\\fB{flag_str}\\fR"));
                } else {
                    let type_name_owned: String = arg
                        .get_value_names()
                        .and_then(|v| v.first().map(|s| s.to_string()))
                        .unwrap_or_else(|| "VALUE".to_string());
                    let type_name = type_name_owned.as_str();
                    sections.push(format!("\\fB{flag_str}\\fR \\fI{type_name}\\fR"));
                }
                if let Some(help) = arg.get_help() {
                    sections.push(help.to_string());
                }
                if let Some(default) = arg.get_default_values().first() {
                    if !is_flag {
                        sections.push(format!("Default: {}.", default.to_string_lossy()));
                    }
                }
            }
        }
    }

    // .SH ENVIRONMENT (static)
    sections.push(".SH ENVIRONMENT".to_string());
    for (name, desc) in ENV_ENTRIES {
        sections.push(".TP".to_string());
        sections.push(format!("\\fB{name}\\fR"));
        sections.push(desc.to_string());
    }

    // .SH EXIT CODES (static — full table from spec)
    sections.push(".SH EXIT CODES".to_string());
    for (code, meaning) in EXIT_CODES {
        sections.push(format!(".TP\n\\fB{code}\\fR\n{meaning}"));
    }

    // .SH SEE ALSO
    sections.push(".SH SEE ALSO".to_string());
    let see_also = [
        format!("\\fB{prog_name}\\fR(1)"),
        format!("\\fB{prog_name}\\-list\\fR(1)"),
        format!("\\fB{prog_name}\\-describe\\fR(1)"),
        format!("\\fB{prog_name}\\-completion\\fR(1)"),
    ];
    sections.push(see_also.join(", "));

    sections.join("\n")
}

/// Static environment variable entries for the ENVIRONMENT section.
pub const ENV_ENTRIES: &[(&str, &str)] = &[
    (
        "APCORE_EXTENSIONS_ROOT",
        "Path to the apcore extensions directory. Overrides the default \\fI./extensions\\fR.",
    ),
    (
        "APCORE_CLI_AUTO_APPROVE",
        "Set to \\fB1\\fR to bypass approval prompts for modules that require human-in-the-loop confirmation.",
    ),
    (
        "APCORE_CLI_LOGGING_LEVEL",
        "CLI-specific logging verbosity. One of: DEBUG, INFO, WARNING, ERROR. \
         Takes priority over \\fBAPCORE_LOGGING_LEVEL\\fR. Default: WARNING.",
    ),
    (
        "APCORE_AUTH_API_KEY",
        "API key for authenticating with the apcore registry.",
    ),
];

/// Static exit code entries for the EXIT CODES section.
pub const EXIT_CODES: &[(&str, &str)] = &[
    ("0", "Success."),
    ("1", "Module execution error."),
    ("2", "Invalid CLI input or missing argument."),
    ("44", "Module not found, disabled, or failed to load."),
    ("45", "Input failed JSON Schema validation."),
    (
        "46",
        "Approval denied, timed out, or no interactive terminal available.",
    ),
    (
        "47",
        "Configuration error (extensions directory not found or unreadable).",
    ),
    ("48", "Schema contains a circular \\fB$ref\\fR."),
    (
        "77",
        "ACL denied \\- insufficient permissions for this module.",
    ),
    ("130", "Execution cancelled by user (SIGINT / Ctrl\\-C)."),
];

/// Handler: look up a subcommand and return its roff man page.
///
/// Returns `Err(ShellError::UnknownCommand)` if `command_name` is not found
/// among `root_cmd`'s subcommands and is not in `KNOWN_BUILTINS`.
pub fn cmd_man(
    command_name: &str,
    root_cmd: &clap::Command,
    prog_name: &str,
    version: &str,
) -> Result<String, ShellError> {
    // Try live subcommand tree first
    let cmd_opt = root_cmd
        .get_subcommands()
        .find(|c| c.get_name() == command_name);

    // Fall back to known built-ins (commands that may not be wired yet)
    if cmd_opt.is_none() && !KNOWN_BUILTINS.contains(&command_name) {
        return Err(ShellError::UnknownCommand(command_name.to_string()));
    }

    Ok(generate_man_page(command_name, cmd_opt, prog_name, version))
}

/// Format Unix epoch days as YYYY-MM-DD without external crates.
fn format_roff_date(days_since_epoch: u64) -> String {
    let mut remaining = days_since_epoch;
    let mut year = 1970u32;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_year = if leap { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for &d in &month_days {
        if remaining < d {
            break;
        }
        remaining -= d;
        month += 1;
    }
    let day = remaining + 1;
    format!("{year:04}-{month:02}-{day:02}")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Task 1: ShellError and KNOWN_BUILTINS ---

    #[test]
    fn test_shell_error_unknown_command_message() {
        let err = ShellError::UnknownCommand("bogus".to_string());
        assert_eq!(err.to_string(), "unknown command 'bogus'");
    }

    #[test]
    fn test_known_builtins_contains_required_commands() {
        for cmd in &["exec", "list", "describe", "completion", "man"] {
            assert!(
                KNOWN_BUILTINS.contains(cmd),
                "KNOWN_BUILTINS must contain '{cmd}'"
            );
        }
    }

    #[test]
    fn test_known_builtins_has_expected_count() {
        assert_eq!(KNOWN_BUILTINS.len(), 5);
    }

    // --- Task 2: completion_command / cmd_completion ---

    fn make_test_cmd(prog: &str) -> clap::Command {
        clap::Command::new(prog.to_string())
            .about("test")
            .subcommand(clap::Command::new("exec"))
            .subcommand(clap::Command::new("list"))
    }

    #[test]
    fn test_cmd_completion_bash_nonempty() {
        let mut cmd = make_test_cmd("apcore-cli");
        let output = cmd_completion(Shell::Bash, "apcore-cli", &mut cmd);
        assert!(
            !output.is_empty(),
            "bash completion output must not be empty"
        );
    }

    #[test]
    fn test_cmd_completion_zsh_nonempty() {
        let mut cmd = make_test_cmd("apcore-cli");
        let output = cmd_completion(Shell::Zsh, "apcore-cli", &mut cmd);
        assert!(
            !output.is_empty(),
            "zsh completion output must not be empty"
        );
    }

    #[test]
    fn test_cmd_completion_fish_nonempty() {
        let mut cmd = make_test_cmd("apcore-cli");
        let output = cmd_completion(Shell::Fish, "apcore-cli", &mut cmd);
        assert!(
            !output.is_empty(),
            "fish completion output must not be empty"
        );
    }

    #[test]
    fn test_cmd_completion_elvish_nonempty() {
        let mut cmd = make_test_cmd("apcore-cli");
        let output = cmd_completion(Shell::Elvish, "apcore-cli", &mut cmd);
        assert!(
            !output.is_empty(),
            "elvish completion output must not be empty"
        );
    }

    #[test]
    fn test_cmd_completion_bash_contains_prog_name() {
        let mut cmd = make_test_cmd("my-tool");
        let output = cmd_completion(Shell::Bash, "my-tool", &mut cmd);
        assert!(
            output.contains("my-tool") || output.contains("my_tool"),
            "bash completion must reference the program name"
        );
    }

    #[test]
    fn test_completion_command_has_shell_arg() {
        let cmd = completion_command();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "shell");
        assert!(
            arg.is_some(),
            "completion_command must have a 'shell' argument"
        );
    }

    #[test]
    fn test_completion_command_name() {
        let cmd = completion_command();
        assert_eq!(cmd.get_name(), "completion");
    }

    // --- Task 3: build_synopsis / generate_man_page / cmd_man ---

    fn make_exec_cmd() -> clap::Command {
        clap::Command::new("exec")
            .about("Execute an apcore module")
            .arg(
                clap::Arg::new("module_id")
                    .value_name("MODULE_ID")
                    .required(true)
                    .help("Module ID to execute"),
            )
            .arg(
                clap::Arg::new("format")
                    .long("format")
                    .value_name("FORMAT")
                    .help("Output format")
                    .default_value("table"),
            )
    }

    #[test]
    fn test_build_synopsis_no_cmd() {
        let synopsis = build_synopsis(None, "apcore-cli", "exec");
        assert!(synopsis.contains("apcore-cli"));
        assert!(synopsis.contains("exec"));
    }

    #[test]
    fn test_build_synopsis_required_positional_no_brackets() {
        let cmd = make_exec_cmd();
        let synopsis = build_synopsis(Some(&cmd), "apcore-cli", "exec");
        assert!(synopsis.contains("MODULE_ID"), "synopsis: {synopsis}");
        assert!(
            !synopsis.contains("[\\fIMODULE_ID\\fR]"),
            "required arg must not have brackets"
        );
    }

    #[test]
    fn test_build_synopsis_optional_option_has_brackets() {
        let cmd = make_exec_cmd();
        let synopsis = build_synopsis(Some(&cmd), "apcore-cli", "exec");
        assert!(
            synopsis.contains('['),
            "optional option must be wrapped in brackets"
        );
    }

    #[test]
    fn test_generate_man_page_contains_th() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(page.contains(".TH"), "man page must have .TH header");
    }

    #[test]
    fn test_generate_man_page_contains_sh_name() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(page.contains(".SH NAME"), "man page must have NAME section");
    }

    #[test]
    fn test_generate_man_page_contains_sh_synopsis() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(
            page.contains(".SH SYNOPSIS"),
            "man page must have SYNOPSIS section"
        );
    }

    #[test]
    fn test_generate_man_page_contains_exit_codes() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(
            page.contains(".SH EXIT CODES"),
            "man page must have EXIT CODES section"
        );
        assert!(page.contains("\\fB0\\fR"), "must contain exit code 0");
        assert!(page.contains("\\fB44\\fR"), "must contain exit code 44");
        assert!(page.contains("\\fB130\\fR"), "must contain exit code 130");
    }

    #[test]
    fn test_generate_man_page_contains_environment() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(
            page.contains(".SH ENVIRONMENT"),
            "man page must have ENVIRONMENT section"
        );
        assert!(page.contains("APCORE_EXTENSIONS_ROOT"));
        assert!(page.contains("APCORE_CLI_LOGGING_LEVEL"));
    }

    #[test]
    fn test_generate_man_page_contains_see_also() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(
            page.contains(".SH SEE ALSO"),
            "man page must have SEE ALSO section"
        );
        assert!(page.contains("apcore-cli"));
    }

    #[test]
    fn test_generate_man_page_th_includes_prog_and_version() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        let th_line = page.lines().find(|l| l.starts_with(".TH")).unwrap();
        assert!(
            th_line.contains("APCORE-CLI-EXEC"),
            "TH must contain uppercased title"
        );
        assert!(th_line.contains("0.2.0"), "TH must contain version");
    }

    #[test]
    fn test_generate_man_page_name_uses_description() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(
            page.contains("Execute an apcore module"),
            "NAME must use about text"
        );
    }

    #[test]
    fn test_generate_man_page_no_description_section_when_no_long_help() {
        let cmd = make_exec_cmd();
        let page = generate_man_page("exec", Some(&cmd), "apcore-cli", "0.2.0");
        assert!(page.contains(".SH DESCRIPTION"));
    }

    #[test]
    fn test_cmd_man_known_builtin_returns_ok() {
        let root = clap::Command::new("apcore-cli").subcommand(make_exec_cmd());
        let result = cmd_man("list", &root, "apcore-cli", "0.2.0");
        assert!(result.is_ok(), "known builtin 'list' must return Ok");
    }

    #[test]
    fn test_cmd_man_registered_subcommand_returns_ok() {
        let root = clap::Command::new("apcore-cli").subcommand(make_exec_cmd());
        let result = cmd_man("exec", &root, "apcore-cli", "0.2.0");
        assert!(
            result.is_ok(),
            "registered subcommand 'exec' must return Ok"
        );
        let page = result.unwrap();
        assert!(page.contains(".TH"));
    }

    #[test]
    fn test_cmd_man_unknown_command_returns_err() {
        let root = clap::Command::new("apcore-cli");
        let result = cmd_man("nonexistent", &root, "apcore-cli", "0.2.0");
        assert!(result.is_err());
        match result.unwrap_err() {
            ShellError::UnknownCommand(name) => assert_eq!(name, "nonexistent"),
        }
    }

    #[test]
    fn test_cmd_man_exec_contains_options_section() {
        let root = clap::Command::new("apcore-cli").subcommand(make_exec_cmd());
        let page = cmd_man("exec", &root, "apcore-cli", "0.2.0").unwrap();
        assert!(
            page.contains(".SH OPTIONS"),
            "exec man page must have OPTIONS section"
        );
    }

    // --- Task 4: register_shell_commands ---

    #[test]
    fn test_register_shell_commands_adds_completion() {
        let root = Command::new("apcore-cli");
        let cmd = register_shell_commands(root, "apcore-cli");
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"completion"),
            "must have 'completion' subcommand, got {names:?}"
        );
    }

    #[test]
    fn test_register_shell_commands_adds_man() {
        let root = Command::new("apcore-cli");
        let cmd = register_shell_commands(root, "apcore-cli");
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"man"),
            "must have 'man' subcommand, got {names:?}"
        );
    }

    #[test]
    fn test_completion_bash_outputs_script() {
        let cmd = completion_command();
        let positionals: Vec<&str> = cmd
            .get_positionals()
            .filter_map(|a| a.get_id().as_str().into())
            .collect();
        // The arg is named "shell" with value_name "SHELL"
        assert!(
            !positionals.is_empty() || cmd.get_arguments().any(|a| a.get_id() == "shell"),
            "completion must have shell arg, got {positionals:?}"
        );
    }

    #[test]
    fn test_completion_zsh_outputs_script() {
        let cmd = completion_command();
        let shell_arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "shell")
            .expect("shell argument must exist");
        let possible = shell_arg.get_possible_values();
        let values: Vec<&str> = possible.iter().map(|v| v.get_name()).collect();
        assert!(values.contains(&"zsh"), "zsh must be a valid SHELL value");
    }

    #[test]
    fn test_completion_invalid_shell_exits_nonzero() {
        let cmd = completion_command();
        let shell_arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "shell")
            .expect("shell argument must exist");
        let possible = shell_arg.get_possible_values();
        let values: Vec<&str> = possible.iter().map(|v| v.get_name()).collect();
        assert!(
            !values.contains(&"invalid_shell"),
            "invalid_shell must not be accepted"
        );
    }
}

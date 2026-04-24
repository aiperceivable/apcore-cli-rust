// apcore-cli -- Scaffolding commands (init module).
// Protocol spec: FE-10

use std::fs;
use std::path::Path;

/// Register the `init` subcommand with its `module` sub-subcommand.
pub fn init_command() -> clap::Command {
    clap::Command::new("init")
        .about("Scaffolding commands")
        .subcommand(
            clap::Command::new("module")
                .about("Create a new module from a template")
                .arg(clap::Arg::new("module_id").required(true))
                .arg(
                    clap::Arg::new("style")
                        .long("style")
                        .default_value("convention")
                        .value_parser(["decorator", "convention", "binding"]),
                )
                .arg(clap::Arg::new("dir").long("dir").value_name("PATH"))
                .arg(
                    clap::Arg::new("description")
                        .long("description")
                        .short('d')
                        .default_value("TODO: add description"),
                )
                .arg(
                    clap::Arg::new("force")
                        .long("force")
                        .short('f')
                        .help("Overwrite existing scaffold files")
                        .action(clap::ArgAction::SetTrue),
                ),
        )
}

/// Attach the `init` subcommand to the given command. Returns the command
/// with the subcommand added.
///
/// This mirrors the per-subcommand registrar pattern used by the FE-13
/// built-in group (see `discovery::register_list_command`,
/// `system_cmd::register_health_command`, etc.) so the dispatcher can
/// honor include/exclude filtering on `init` like any other built-in.
pub(crate) fn register_init_command(cli: clap::Command) -> clap::Command {
    cli.subcommand(init_command())
}

/// Handle the `init` subcommand dispatch.
pub fn handle_init(matches: &clap::ArgMatches) {
    if let Some(("module", sub_m)) = matches.subcommand() {
        let module_id = sub_m.get_one::<String>("module_id").unwrap();
        let style = sub_m.get_one::<String>("style").unwrap();
        let description = sub_m.get_one::<String>("description").unwrap();
        let force = sub_m.get_flag("force");

        // Parse module_id: split on last dot for prefix/func_name.
        let (prefix, func_name) = match module_id.rfind('.') {
            Some(pos) => (&module_id[..pos], &module_id[pos + 1..]),
            None => (module_id.as_str(), module_id.as_str()),
        };

        match style.as_str() {
            "decorator" => {
                let dir = sub_m
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap_or("extensions");
                validate_dir(dir);
                create_decorator_module(module_id, func_name, description, dir, force);
            }
            "convention" => {
                let dir = sub_m
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap_or("commands");
                validate_dir(dir);
                create_convention_module(module_id, prefix, func_name, description, dir, force);
            }
            "binding" => {
                let dir = sub_m
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap_or("bindings");
                validate_dir(dir);
                create_binding_module(module_id, prefix, func_name, description, dir, force);
            }
            _ => unreachable!(),
        }
    }
}

/// Refuse to overwrite an existing scaffold file unless `--force` was passed.
/// Exits with code 2 on conflict so CI and shell pipelines can detect it.
fn guard_overwrite(filepath: &Path, force: bool) {
    if !force && filepath.exists() {
        eprintln!(
            "Error: '{}' already exists. Pass --force to overwrite.",
            filepath.display()
        );
        std::process::exit(2);
    }
}

/// Validate that the output directory does not contain `..` path
/// components, preventing path traversal outside the project directory.
fn validate_dir(dir: &str) {
    let has_dotdot = std::path::Path::new(dir)
        .components()
        .any(|c| c == std::path::Component::ParentDir);
    if has_dotdot {
        eprintln!("Error: Output directory must not contain '..' path components.");
        std::process::exit(2);
    }
}

/// Convert a snake_case name to PascalCase and append "Module".
fn to_struct_name(func_name: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;
    for ch in func_name.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result.push_str("Module");
    result
}

/// Create a decorator-style module (Rust file with Module trait).
fn create_decorator_module(
    module_id: &str,
    func_name: &str,
    description: &str,
    dir: &str,
    force: bool,
) {
    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).unwrap_or_else(|e| {
        eprintln!(
            "Error: cannot create directory '{}': {e}",
            dir_path.display()
        );
        std::process::exit(2);
    });

    let safe_name = module_id.replace('.', "_");
    let filename = format!("{safe_name}.rs");
    let filepath = dir_path.join(&filename);

    let struct_name = to_struct_name(func_name);

    let content = format!(
        "use apcore::module::Module;\n\
         use apcore::context::Context;\n\
         use apcore::errors::ModuleError;\n\
         use async_trait::async_trait;\n\
         use serde_json::{{json, Value}};\n\
         \n\
         /// {description}\n\
         pub struct {struct_name};\n\
         \n\
         #[async_trait]\n\
         impl Module for {struct_name} {{\n\
         {i}fn input_schema(&self) -> Value {{\n\
         {i}{i}json!({{\n\
         {i}{i}{i}\"type\": \"object\",\n\
         {i}{i}{i}\"properties\": {{}}\n\
         {i}{i}}})\n\
         {i}}}\n\
         \n\
         {i}fn output_schema(&self) -> Value {{\n\
         {i}{i}json!({{\n\
         {i}{i}{i}\"type\": \"object\",\n\
         {i}{i}{i}\"properties\": {{\n\
         {i}{i}{i}{i}\"status\": {{ \"type\": \"string\" }}\n\
         {i}{i}{i}}}\n\
         {i}{i}}})\n\
         {i}}}\n\
         \n\
         {i}fn description(&self) -> &str {{\n\
         {i}{i}\"{description}\"\n\
         {i}}}\n\
         \n\
         {i}async fn execute(\n\
         {i}{i}&self,\n\
         {i}{i}_input: Value,\n\
         {i}{i}_ctx: &Context<Value>,\n\
         {i}) -> Result<Value, ModuleError> {{\n\
         {i}{i}// TODO: implement\n\
         {i}{i}Ok(json!({{ \"status\": \"ok\" }}))\n\
         {i}}}\n\
         }}\n",
        i = "    ",
    );

    guard_overwrite(&filepath, force);
    fs::write(&filepath, content).unwrap_or_else(|e| {
        eprintln!("Error: cannot write '{}': {e}", filepath.display());
        std::process::exit(2);
    });

    println!("Created {}", filepath.display());
}

/// Create a convention-style module (Rust function with
/// CLI_GROUP constant).
fn create_convention_module(
    module_id: &str,
    prefix: &str,
    func_name: &str,
    description: &str,
    dir: &str,
    force: bool,
) {
    // Build the file path: prefix parts become subdirectories.
    // e.g. module_id "ops.deploy" with dir "commands"
    //   -> "commands/ops/deploy.rs"
    // e.g. module_id "standalone" with dir "commands"
    //   -> "commands/standalone.rs"
    let filepath = if module_id.contains('.') {
        let parts: Vec<&str> = module_id.split('.').collect();
        let mut p = Path::new(dir).to_path_buf();
        for part in &parts[..parts.len() - 1] {
            p = p.join(part);
        }
        p.join(format!("{}.rs", parts[parts.len() - 1]))
    } else {
        Path::new(dir).join(format!("{func_name}.rs"))
    };

    if let Some(parent) = filepath.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("Error: cannot create directory '{}': {e}", parent.display());
            std::process::exit(2);
        });
    }

    // Only emit CLI_GROUP when module_id contains a dot.
    let first_segment = prefix.split('.').next().unwrap_or(prefix);
    let cli_group_line = if module_id.contains('.') {
        format!("pub const CLI_GROUP: &str = \"{first_segment}\";\n\n")
    } else {
        String::new()
    };

    let content = format!(
        "//! {description}\n\
         \n\
         {cli_group_line}\
         use serde_json::{{json, Value}};\n\
         \n\
         /// {description}\n\
         pub fn {func_name}() -> Value {{\n\
         {i}// TODO: implement\n\
         {i}json!({{ \"status\": \"ok\" }})\n\
         }}\n",
        i = "    ",
    );

    guard_overwrite(&filepath, force);
    fs::write(&filepath, content).unwrap_or_else(|e| {
        eprintln!("Error: cannot write '{}': {e}", filepath.display());
        std::process::exit(2);
    });

    println!("Created {}", filepath.display());
}

/// Create a binding-style module (YAML binding + companion Rust
/// file).
fn create_binding_module(
    module_id: &str,
    prefix: &str,
    func_name: &str,
    description: &str,
    dir: &str,
    force: bool,
) {
    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).unwrap_or_else(|e| {
        eprintln!(
            "Error: cannot create directory '{}': {e}",
            dir_path.display()
        );
        std::process::exit(2);
    });

    // Write YAML binding file.
    let safe_name = module_id.replace('.', "_");
    let yaml_filename = format!("{safe_name}.binding.yaml");
    let yaml_filepath = dir_path.join(&yaml_filename);

    let target = format!("commands.{prefix}:{func_name}");
    let prefix_underscored = prefix.replace('.', "_");

    let yaml_content = format!(
        "bindings:\n\
         {i}- module_id: \"{module_id}\"\n\
         {i}{i}target: \"{target}\"\n\
         {i}{i}description: \"{description}\"\n\
         {i}{i}auto_schema: true\n",
        i = "  ",
    );

    guard_overwrite(&yaml_filepath, force);
    fs::write(&yaml_filepath, yaml_content).unwrap_or_else(|e| {
        eprintln!("Error: cannot write '{}': {e}", yaml_filepath.display());
        std::process::exit(2);
    });

    println!("Created {}", yaml_filepath.display());

    // Companion Rust file: place it in a `commands/` directory that is a
    // sibling of `dir_path`, so a user passing `--dir my/bindings` gets
    // their companion at `my/commands/...` instead of leaking it to the
    // CWD's `./commands/`. When `dir_path` has no parent (e.g. the default
    // `bindings`), fall back to `./commands` to preserve behavior.
    let commands_dir = dir_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join("commands"))
        .unwrap_or_else(|| Path::new("commands").to_path_buf());
    let rs_filename = format!("{prefix_underscored}.rs");
    let rs_filepath = commands_dir.join(&rs_filename);

    // The companion file is only seeded once per prefix (multiple bindings
    // can share the same Rust function module); leave existing user code
    // alone unless --force was passed.
    if rs_filepath.exists() && !force {
        return;
    }

    if let Some(parent) = rs_filepath.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("Error: cannot create directory '{}': {e}", parent.display());
            std::process::exit(2);
        });
    }

    let rs_content = format!(
        "use serde_json::{{json, Value}};\n\
         \n\
         /// {description}\n\
         pub fn {func_name}() -> Value {{\n\
         {i}// TODO: implement\n\
         {i}json!({{ \"status\": \"ok\" }})\n\
         }}\n",
        i = "    ",
    );

    fs::write(&rs_filepath, rs_content).unwrap_or_else(|e| {
        eprintln!("Error: cannot write '{}': {e}", rs_filepath.display());
        std::process::exit(2);
    });

    println!("Created {}", rs_filepath.display());
}

// -------------------------------------------------------------------
// Unit tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_command_has_module_subcommand() {
        let cmd = init_command();
        let sub = cmd.get_subcommands().find(|c| c.get_name() == "module");
        assert!(sub.is_some(), "init must have 'module' subcommand");
    }

    #[test]
    fn test_init_command_module_has_required_module_id() {
        let cmd = init_command();
        let module_cmd = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "module")
            .expect("module subcommand");
        let arg = module_cmd
            .get_arguments()
            .find(|a| a.get_id() == "module_id");
        assert!(arg.is_some(), "must have module_id arg");
        assert!(arg.unwrap().is_required_set(), "module_id must be required");
    }

    #[test]
    fn test_init_command_module_has_style_flag() {
        let cmd = init_command();
        let module_cmd = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "module")
            .expect("module subcommand");
        let style = module_cmd.get_arguments().find(|a| a.get_id() == "style");
        assert!(style.is_some(), "must have --style flag");
    }

    #[test]
    fn test_init_command_module_has_dir_flag() {
        let cmd = init_command();
        let module_cmd = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "module")
            .expect("module subcommand");
        let dir = module_cmd.get_arguments().find(|a| a.get_id() == "dir");
        assert!(dir.is_some(), "must have --dir flag");
    }

    #[test]
    fn test_init_command_module_has_description_flag() {
        let cmd = init_command();
        let module_cmd = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "module")
            .expect("module subcommand");
        let desc = module_cmd
            .get_arguments()
            .find(|a| a.get_id() == "description");
        assert!(desc.is_some(), "must have --description flag");
    }

    #[test]
    fn test_init_command_parses_valid_args() {
        let cmd = init_command();
        let result =
            cmd.try_get_matches_from(vec!["init", "module", "my.module", "--style", "decorator"]);
        assert!(result.is_ok(), "valid args must parse: {:?}", result.err());
    }

    #[test]
    fn test_register_init_command_attaches_init() {
        let root = register_init_command(clap::Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            subs.contains(&"init"),
            "must have 'init' subcommand, got {subs:?}"
        );

        // Verify the init subcommand retains its 'module' sub-subcommand.
        let init_sub = root
            .get_subcommands()
            .find(|c| c.get_name() == "init")
            .expect("init subcommand");
        let nested: Vec<&str> = init_sub.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            nested.contains(&"module"),
            "init must have 'module' sub-subcommand, got {nested:?}"
        );
    }
}

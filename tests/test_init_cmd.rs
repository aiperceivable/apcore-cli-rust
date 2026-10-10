// Integration tests for init_cmd (FE-10).

use std::fs;

use tempfile::TempDir;

#[test]
fn test_convention_style_creates_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cmds");
    let dir_str = dir.to_str().unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "greet",
            "--style",
            "convention",
            "--dir",
            dir_str,
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let file = dir.join("greet.rs");
    assert!(file.exists(), "convention file must be created");
    let content = fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("pub fn greet("),
        "must contain function definition"
    );
}

#[test]
fn test_decorator_style_creates_file_with_module() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("ext");
    let dir_str = dir.to_str().unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "math.add",
            "--style",
            "decorator",
            "--dir",
            dir_str,
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let file = dir.join("math_add.rs");
    assert!(file.exists(), "decorator file must be created");
    let content = fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("impl Module for"),
        "must contain impl Module"
    );
}

#[test]
fn test_binding_style_creates_yaml() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("bindings");
    let dir_str = dir.to_str().unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "text.upper",
            "--style",
            "binding",
            "--dir",
            dir_str,
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let yaml_file = dir.join("text_upper.binding.yaml");
    assert!(yaml_file.exists(), "YAML binding must be created");
    let yaml_content = fs::read_to_string(&yaml_file).unwrap();
    assert!(
        yaml_content.contains("text.upper"),
        "YAML must contain module id"
    );

    for strict in [false, true] {
        let modules = apcore_toolkit::BindingLoader::new()
            .load(&yaml_file, strict, false)
            .expect("generated bindings must load with toolkit 0.14.0");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].module_id, "text.upper");
        assert_eq!(
            modules[0].input_schema,
            serde_json::json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            modules[0].output_schema["properties"]["status"],
            serde_json::json!({"type": "string"})
        );
    }

    // Companion Rust file lives in a sibling `commands/` dir of `--dir`,
    // so passing `--dir <tmp>/bindings` puts the companion in
    // `<tmp>/commands/text.rs` rather than leaking it to the CWD.
    let rs_file = tmp.path().join("commands").join("text.rs");
    assert!(
        rs_file.exists(),
        "companion Rust file must live next to bindings dir, got missing {}",
        rs_file.display()
    );
}

#[test]
fn test_convention_dotted_id_has_cli_group() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cmds2");
    let dir_str = dir.to_str().unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "math.add",
            "--style",
            "convention",
            "--dir",
            dir_str,
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let file = dir.join("math").join("add.rs");
    let content = fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("pub const CLI_GROUP: &str = \"math\""),
        "dotted module_id must produce CLI_GROUP"
    );
}

#[test]
fn test_force_flag_in_command_definition() {
    // Regression for review #4: --force / -f must be exposed on `init module`
    // so callers can opt into overwriting an existing scaffold.
    let cmd = apcore_cli::init_cmd::init_command();
    let module_cmd = cmd
        .get_subcommands()
        .find(|c| c.get_name() == "module")
        .expect("module subcommand");
    let force = module_cmd
        .get_arguments()
        .find(|a| a.get_id() == "force")
        .expect("must expose --force flag");
    assert_eq!(force.get_short(), Some('f'));
}

#[test]
fn test_force_overwrites_existing_companion() {
    // Regression for review #4 happy path: --force replaces the companion
    // .rs even when one already exists.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("bindings");
    let dir_str = dir.to_str().unwrap();

    let companion = tmp.path().join("commands").join("greeting.rs");
    fs::create_dir_all(companion.parent().unwrap()).unwrap();
    fs::write(&companion, "// pre-existing user code").unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "greeting.hello",
            "--style",
            "binding",
            "--dir",
            dir_str,
            "--force",
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let after = fs::read_to_string(&companion).unwrap();
    assert!(
        after.contains("pub fn hello("),
        "--force must replace the existing companion, got: {after}"
    );
}

#[test]
fn test_description_flag() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("ext2");
    let dir_str = dir.to_str().unwrap();

    let cmd = apcore_cli::init_cmd::init_command();
    let matches = cmd
        .try_get_matches_from(vec![
            "init",
            "module",
            "demo.hello",
            "--style",
            "decorator",
            "--dir",
            dir_str,
            "-d",
            "My custom description",
        ])
        .unwrap();
    apcore_cli::init_cmd::handle_init(&matches);

    let file = dir.join("demo_hello.rs");
    let content = fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("My custom description"),
        "custom description must appear in file"
    );
}

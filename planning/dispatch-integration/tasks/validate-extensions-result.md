# Task: validate_extensions_dir returns Result

## Goal

Refactor `validate_extensions_dir` in `main.rs` from calling `std::process::exit(47)` directly to returning `Result<(), String>`. This makes the function testable and moves exit-code decisions to the caller.

## Files Involved

- `src/main.rs` -- `validate_extensions_dir` function and its call site in `build_cli_command`

## Steps (TDD)

1. **Write tests** in `src/main.rs::tests`:
   ```rust
   #[test]
   fn test_validate_extensions_dir_nonexistent_returns_err() {
       let result = validate_extensions_dir("/nonexistent/path/xxx");
       assert!(result.is_err());
       assert!(result.unwrap_err().contains("not found"));
   }

   #[test]
   fn test_validate_extensions_dir_valid_returns_ok() {
       // Use a temp dir or std::env::temp_dir()
       let dir = std::env::temp_dir();
       let result = validate_extensions_dir(dir.to_str().unwrap());
       assert!(result.is_ok());
   }
   ```

2. **Change signature** from `fn validate_extensions_dir(ext_dir: &str)` to:
   ```rust
   fn validate_extensions_dir(ext_dir: &str) -> Result<(), String>
   ```

3. **Replace `process::exit` calls with `Err(...)`**:
   - Non-existent path: `return Err(format!("Extensions directory not found: '{}'. ...", ext_dir))`
   - Unreadable path: `return Err(format!("Cannot read extensions directory: '{}'. ...", ext_dir))`
   - Success: `Ok(())`

4. **Update caller in `build_cli_command`**:
   ```rust
   if validate {
       if let Err(msg) = validate_extensions_dir(&ext_dir) {
           eprintln!("Error: {msg}");
           std::process::exit(EXIT_CONFIG_NOT_FOUND);
       }
   }
   ```

5. **Run tests**: `cargo test`.

6. **Run clippy**: `cargo clippy -- -D warnings`.

## Acceptance Criteria

- `validate_extensions_dir` returns `Result<(), String>` and never calls `process::exit`.
- `build_cli_command` handles the error by printing and exiting (behavior unchanged from user perspective).
- New unit tests cover both the error and success paths.
- All existing tests pass.

## Dependencies

None. However, completing this before `dispatch-wiring` makes the dispatch path easier to test.

## Estimated Time

~30 minutes

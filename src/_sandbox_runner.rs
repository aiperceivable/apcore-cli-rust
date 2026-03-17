// apcore-cli — Internal sandbox runner entry point.
// This module is NOT part of the public API. It is invoked as a subprocess
// by the Sandbox security layer to execute modules in isolation.
// Protocol spec: SEC-04 (Sandbox)

use serde_json::Value;

/// Entry point for the sandboxed subprocess.
///
/// Reads `module_id` from `argv[2]` (position after `apcore-cli --internal-sandbox-runner`)
/// and `input_data` as JSON from stdin, calls the executor, and writes the
/// JSON result to stdout.
///
/// Exit codes mirror the main CLI conventions (0, 1, 44, 45, …).
pub async fn run_sandbox_subprocess() -> Result<(), anyhow::Error> {
    use tokio::io::AsyncReadExt;

    let module_id = std::env::args()
        .nth(2)
        .ok_or_else(|| anyhow::anyhow!("sandbox runner: missing module_id argument"))?;

    // Read JSON input from stdin.
    let mut stdin_buf = String::new();
    tokio::io::stdin().read_to_string(&mut stdin_buf).await?;
    let input_data: Value = serde_json::from_str(&stdin_buf)?;

    // Instantiate executor via the apcore registry.
    let registry = apcore::Registry::new();
    let config = apcore::Config::default();
    let executor = apcore::Executor::new(registry, config);
    let result = executor.call(&module_id, input_data, None, None).await?;

    // Write JSON result to stdout.
    let encoded = encode_result(&result);
    print!("{encoded}");
    Ok(())
}

/// Serialise the sandbox result for IPC.
pub fn encode_result(result: &Value) -> String {
    serde_json::to_string(result).unwrap_or_else(|_| "null".to_string())
}

/// Deserialise the sandbox result received by the parent process.
pub fn decode_result(raw: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(raw)
}

// apcore-cli — Integration tests for AuthProvider.
// Protocol spec: SEC-02

use std::collections::HashMap;

use apcore_cli::config::ConfigResolver;
use apcore_cli::security::auth::{AuthProvider, AuthenticationError};

/// Serialize all tests that touch APCORE_AUTH_API_KEY to prevent data races
/// when tests run in parallel.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn make_empty_resolver() -> ConfigResolver {
    ConfigResolver::new(None, None)
}

#[test]
fn test_get_api_key_from_env_var() {
    // APCORE_AUTH_API_KEY must be returned when set.
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: test-only env manipulation, serialized via ENV_LOCK.
    unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "env-key-abc") };
    let provider = AuthProvider::new(make_empty_resolver());
    let key = provider.get_api_key();
    // SAFETY: cleanup regardless of assertion outcome.
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    assert_eq!(key.unwrap(), Some("env-key-abc".to_string()));
}

#[test]
fn test_get_api_key_missing_returns_error() {
    // When no key is available, get_api_key() must return Ok(None).
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: test-only env manipulation, serialized via ENV_LOCK.
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    assert_eq!(provider.get_api_key().unwrap(), None);
}

#[test]
fn test_authenticate_request_adds_bearer_header() {
    // authenticate_request (HashMap-based) must insert Authorization when key is available.
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: test-only env manipulation, serialized via ENV_LOCK.
    unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "bearer-test-key") };
    let provider = AuthProvider::new(make_empty_resolver());
    let mut headers = std::collections::HashMap::new();
    let result = provider.authenticate_request(&mut headers);
    // SAFETY: cleanup.
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    assert!(
        result.is_ok(),
        "authenticate_request must succeed when key is set"
    );
    assert_eq!(
        headers.get("Authorization").map(|s| s.as_str()),
        Some("Bearer bearer-test-key"),
        "Authorization header must be set"
    );
}

#[test]
fn test_apply_to_reqwest_injects_bearer_header() {
    // apply_to_reqwest must succeed when a key is available.
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "reqwest-key") };
    let provider = AuthProvider::new(make_empty_resolver());
    let client = reqwest::Client::new();
    let builder = client.get("https://example.com");
    let result = provider.apply_to_reqwest(builder);
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    assert!(
        result.is_ok(),
        "apply_to_reqwest must succeed when key is set"
    );
}

#[test]
fn test_check_status_code_200_ok() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    assert!(provider.check_status_code(200).is_ok());
}

#[test]
fn test_check_status_code_401_returns_invalid_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    let err = provider.check_status_code(401).unwrap_err();
    assert!(matches!(err, AuthenticationError::InvalidApiKey));
}

#[test]
fn test_check_status_code_403_returns_invalid_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    let err = provider.check_status_code(403).unwrap_err();
    assert!(matches!(err, AuthenticationError::InvalidApiKey));
}

#[test]
fn test_check_status_code_500_passes_through() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    assert!(provider.check_status_code(500).is_ok());
}

#[test]
fn test_error_messages_match_spec() {
    let missing = AuthenticationError::MissingApiKey;
    assert!(
        missing.to_string().contains("APCORE_AUTH_API_KEY"),
        "MissingApiKey message must mention the env var"
    );
    let invalid = AuthenticationError::InvalidApiKey;
    assert!(
        invalid.to_string().contains("Authentication failed"),
        "InvalidApiKey message must say 'Authentication failed', got: {}",
        invalid
    );
}

// Regression test for A-D-008: CLI flag must take precedence over env var.
#[test]
fn test_cli_flag_takes_precedence_over_env_var() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Set env var to "env-key"
    unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "env-key") };

    // Supply "cli-key" via the resolver's cli_flags map
    let mut flags = HashMap::new();
    flags.insert("--api-key".to_string(), Some("cli-key".to_string()));
    let resolver = ConfigResolver::new(Some(flags), None);
    let provider = AuthProvider::new(resolver);
    let result = provider.get_api_key();

    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };

    // CLI flag must win — should be "cli-key", not "env-key"
    assert_eq!(
        result.unwrap(),
        Some("cli-key".to_string()),
        "CLI --api-key flag must take precedence over APCORE_AUTH_API_KEY env var"
    );
}

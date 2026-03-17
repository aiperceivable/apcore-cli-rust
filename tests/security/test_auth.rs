// apcore-cli — Integration tests for AuthProvider.
// Protocol spec: SEC-02

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
    assert_eq!(key, Some("env-key-abc".to_string()));
}

#[test]
fn test_get_api_key_missing_returns_error() {
    // When no key is available, get_api_key() must return None.
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: test-only env manipulation, serialized via ENV_LOCK.
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    let provider = AuthProvider::new(make_empty_resolver());
    assert_eq!(provider.get_api_key(), None);
}

#[test]
fn test_authenticate_request_adds_bearer_header() {
    // authenticate_request must succeed when a key is available.
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: test-only env manipulation, serialized via ENV_LOCK.
    unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "bearer-test-key") };
    let provider = AuthProvider::new(make_empty_resolver());
    let client = reqwest::Client::new();
    let builder = client.get("https://example.com");
    let result = provider.authenticate_request(builder);
    // SAFETY: cleanup.
    unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };
    assert!(
        result.is_ok(),
        "authenticate_request must succeed when key is set"
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

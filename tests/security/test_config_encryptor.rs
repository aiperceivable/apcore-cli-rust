// apcore-cli — Integration tests for ConfigEncryptor.
// Protocol spec: SEC-03

use apcore_cli::security::config_encryptor::{ConfigDecryptionError, ConfigEncryptor};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

/// Returns a ConfigEncryptor that always uses AES encryption, bypassing the
/// OS keyring. Safe to use in headless CI environments.
fn aes_enc() -> ConfigEncryptor {
    ConfigEncryptor::new_forced_aes()
}

#[test]
fn test_store_and_retrieve_roundtrip() {
    // store("key", "val") then retrieve(<token>, "key") must return "val".
    let enc = aes_enc();
    let token = enc
        .store("auth.api_key", "my-secret")
        .expect("store must succeed");
    let result = enc
        .retrieve(&token, "auth.api_key")
        .expect("retrieve must succeed");
    assert_eq!(result, "my-secret");
}

#[test]
fn test_retrieve_missing_key_returns_error() {
    // Attempting to retrieve a keyring: reference that was never stored must
    // yield a KeyringError (or fall back to AuthTagMismatch if the enc: path
    // is taken). We test the "keyring:nonexistent" path explicitly.
    let enc = aes_enc();
    // Use the keyring: prefix to trigger the keyring lookup path; the entry
    // won't exist, so KeyringError must be returned.
    let result = enc.retrieve("keyring:__apcore_test_missing_9f3d__", "auth.api_key");
    assert!(
        matches!(result, Err(ConfigDecryptionError::KeyringError(_))),
        "expected KeyringError for missing keyring entry, got {result:?}"
    );
}

#[test]
fn test_tampered_ciphertext_returns_auth_tag_error() {
    // Corrupting the ciphertext must yield AuthTagMismatch on retrieve.
    let enc = aes_enc();
    // Build a syntactically valid but cryptographically corrupt enc: token:
    // 40 bytes (12 nonce + 16 tag + 12 ciphertext) with a corrupted tag byte.
    let mut bad = vec![0u8; 40];
    bad[12] ^= 0xFF; // corrupt tag byte
    let config_value = format!("enc:{}", B64.encode(&bad));
    let result = enc.retrieve(&config_value, "some.key");
    assert!(
        matches!(result, Err(ConfigDecryptionError::AuthTagMismatch)),
        "expected AuthTagMismatch for tampered ciphertext, got {result:?}"
    );
}

#[test]
fn test_different_services_are_independent() {
    // Values stored under different keys must not interfere with each other.
    let enc = aes_enc();
    let token_a = enc.store("service.key_a", "value-a").expect("store a");
    let token_b = enc.store("service.key_b", "value-b").expect("store b");
    let result_a = enc.retrieve(&token_a, "service.key_a").expect("retrieve a");
    let result_b = enc.retrieve(&token_b, "service.key_b").expect("retrieve b");
    assert_eq!(result_a, "value-a");
    assert_eq!(result_b, "value-b");
    // Tokens must differ (different ciphertexts due to random nonce).
    assert_ne!(token_a, token_b);
}

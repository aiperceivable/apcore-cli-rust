// apcore-cli — Encrypted config storage.
// Protocol spec: SEC-03 (ConfigEncryptor, ConfigDecryptionError)

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use gethostname::gethostname;
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SERVICE_NAME: &str = "apcore-cli";
/// Legacy static salt used by `enc:` (v1) tokens — kept for decryption
/// backward compatibility only.  New encryptions use `enc:v2:` with a
/// per-encryption random salt embedded in the wire bytes.
const PBKDF2_SALT_V1: &[u8] = b"apcore-cli-config-v1";
/// OWASP 2026 minimum for PBKDF2-HMAC-SHA256.
const PBKDF2_ITERATIONS: u32 = 600_000;
/// Minimum v1 wire-format length: 12-byte nonce + 16-byte tag.
const MIN_WIRE_LEN_V1: usize = 28;
/// Random salt length prepended to v2 wire bytes.
const PBKDF2_SALT_LEN_V2: usize = 16;
/// Minimum v2 wire-format length: 16-byte salt + 12-byte nonce + 16-byte tag.
const MIN_WIRE_LEN_V2: usize = PBKDF2_SALT_LEN_V2 + 28;

// ---------------------------------------------------------------------------
// ConfigDecryptionError
// ---------------------------------------------------------------------------

/// Errors produced by decryption or key-derivation operations.
#[derive(Debug, Error)]
pub enum ConfigDecryptionError {
    /// The ciphertext is malformed or has been tampered with.
    #[error("decryption failed: authentication tag mismatch or corrupt data")]
    AuthTagMismatch,

    /// The stored data was not valid UTF-8 after decryption.
    #[error("decrypted data is not valid UTF-8")]
    InvalidUtf8,

    /// Keyring access failed.
    #[error("keyring error: {0}")]
    KeyringError(String),

    /// Key-derivation failed.
    #[error("key derivation error: {0}")]
    KdfError(String),
}

// ---------------------------------------------------------------------------
// ConfigEncryptor
// ---------------------------------------------------------------------------

/// AES-GCM encrypted config store backed by the system keyring.
///
/// Uses PBKDF2-HMAC-SHA256 for key derivation from a machine-specific
/// `hostname:username` material, and AES-256-GCM for authenticated encryption.
///
/// Wire format for AES-encrypted values:
///   `enc:<base64(nonce[12] || tag[16] || ciphertext)>`
///
/// Keyring-stored values are referenced as:
///   `keyring:<key>`
#[derive(Default)]
pub struct ConfigEncryptor {
    /// When `true`, skip the OS keyring probe and always use AES encryption.
    /// Intended for unit tests running in headless/CI environments.
    _force_aes: bool,
}

impl ConfigEncryptor {
    /// Create a new `ConfigEncryptor` using the OS keyring when available.
    pub fn new() -> Result<Self, ConfigDecryptionError> {
        Ok(Self::default())
    }

    /// Create a `ConfigEncryptor` that always uses AES encryption, bypassing
    /// the OS keyring. Intended for use in tests running in headless/CI environments.
    /// Gated behind the `test-support` feature so it is excluded from production builds.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_forced_aes() -> Self {
        Self { _force_aes: true }
    }

    /// Wrapper for `_keyring_available()` for use in integration tests.
    #[allow(dead_code)]
    pub(crate) fn keyring_available(&self) -> bool {
        self._keyring_available()
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Persist `value` for `key`.
    ///
    /// Tries the OS keyring first. On failure (headless / CI) falls back to
    /// AES-256-GCM file encryption.
    ///
    /// Returns a config-file token:
    /// - `"keyring:<key>"` when stored in the OS keyring.
    /// - `"enc:<base64>"` when stored as an encrypted blob.
    ///
    /// # Security note
    ///
    /// The `enc:` fallback path derives its encryption key from the machine's
    /// hostname and the current username. This protects against casual file
    /// browsing but **not** against targeted attacks by co-tenants on shared
    /// systems who know both values. For sensitive credentials (API keys,
    /// tokens), prefer the `keyring:` path (OS keyring) when available, or
    /// use environment variables instead of config file storage.
    pub fn store(&self, key: &str, value: &str) -> Result<String, ConfigDecryptionError> {
        if self._keyring_available() {
            let entry = keyring::Entry::new(SERVICE_NAME, key)
                .map_err(|e| ConfigDecryptionError::KeyringError(e.to_string()))?;
            entry
                .set_password(value)
                .map_err(|e| ConfigDecryptionError::KeyringError(e.to_string()))?;
            Ok(format!("keyring:{key}"))
        } else {
            tracing::warn!("OS keyring unavailable. Using file-based encryption.");
            let ciphertext = self._aes_encrypt_v2(value)?;
            Ok(format!("enc:v2:{}", B64.encode(&ciphertext)))
        }
    }

    /// Retrieve the plaintext for a config value token.
    ///
    /// Handles four formats:
    /// - `"keyring:<ref>"` — fetch from OS keyring.
    /// - `"enc:v2:<base64>"` — v2: per-encryption random salt (PBKDF2 600k rounds).
    /// - `"enc:<base64>"` — v1 legacy: static PBKDF2 salt (100k rounds, read-only).
    /// - anything else — return as-is (plain passthrough).
    pub fn retrieve(&self, config_value: &str, key: &str) -> Result<String, ConfigDecryptionError> {
        if let Some(ref_key) = config_value.strip_prefix("keyring:") {
            let entry = keyring::Entry::new(SERVICE_NAME, ref_key)
                .map_err(|e| ConfigDecryptionError::KeyringError(e.to_string()))?;
            entry.get_password().map_err(|e| match e {
                keyring::Error::NoEntry => ConfigDecryptionError::KeyringError(format!(
                    "Keyring entry not found for '{ref_key}'."
                )),
                other => ConfigDecryptionError::KeyringError(other.to_string()),
            })
        } else if let Some(b64_data) = config_value.strip_prefix("enc:v2:") {
            let data = B64
                .decode(b64_data)
                .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
            self._aes_decrypt_v2(&data).map_err(|e| match e {
                ConfigDecryptionError::AuthTagMismatch => ConfigDecryptionError::AuthTagMismatch,
                other => ConfigDecryptionError::KeyringError(format!(
                    "Failed to decrypt configuration value '{key}'. \
                     Re-configure with 'apcore-cli config set {key}'. Cause: {other}"
                )),
            })
        } else if let Some(b64_data) = config_value.strip_prefix("enc:") {
            let data = B64
                .decode(b64_data)
                .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
            self._aes_decrypt_v1(&data).map_err(|e| match e {
                ConfigDecryptionError::AuthTagMismatch => ConfigDecryptionError::AuthTagMismatch,
                other => ConfigDecryptionError::KeyringError(format!(
                    "Failed to decrypt configuration value '{key}'. \
                     Re-configure with 'apcore-cli config set {key}'. Cause: {other}"
                )),
            })
        } else {
            Ok(config_value.to_string())
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Returns `true` when the OS keyring is accessible.
    fn _keyring_available(&self) -> bool {
        if self._force_aes {
            return false;
        }
        let entry = match keyring::Entry::new(SERVICE_NAME, "__apcore_probe__") {
            Ok(e) => e,
            Err(_) => return false,
        };
        matches!(entry.get_password(), Ok(_) | Err(keyring::Error::NoEntry))
    }

    /// Derive a 32-byte AES key via PBKDF2-HMAC-SHA256.
    ///
    /// Key material precedence (matching Python/TS parity):
    /// 1. `APCORE_CLI_CONFIG_PASSPHRASE` env var if set and non-empty.
    /// 2. `hostname:username` fallback.
    fn _derive_key_with_salt(&self, salt: &[u8]) -> Result<[u8; 32], ConfigDecryptionError> {
        let material = if let Ok(passphrase) = std::env::var("APCORE_CLI_CONFIG_PASSPHRASE") {
            if !passphrase.is_empty() {
                passphrase
            } else {
                let hostname = gethostname()
                    .into_string()
                    .unwrap_or_else(|_| "unknown".to_string());
                let username = std::env::var("USER")
                    .or_else(|_| std::env::var("LOGNAME"))
                    .unwrap_or_else(|_| "unknown".to_string());
                format!("{hostname}:{username}")
            }
        } else {
            let hostname = gethostname()
                .into_string()
                .unwrap_or_else(|_| "unknown".to_string());
            let username = std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "unknown".to_string());
            format!("{hostname}:{username}")
        };
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha256>(material.as_bytes(), salt, PBKDF2_ITERATIONS, &mut key);
        Ok(key)
    }

    /// Encrypt `plaintext` and return v2 wire bytes.
    ///
    /// Wire format: `salt[16] || nonce[12] || tag[16] || ciphertext`.
    /// A 16-byte random salt is generated per encryption; it is embedded in
    /// the output so no external state is required for decryption.
    pub(crate) fn _aes_encrypt_v2(
        &self,
        plaintext: &str,
    ) -> Result<Vec<u8>, ConfigDecryptionError> {
        let mut salt_bytes = [0u8; PBKDF2_SALT_LEN_V2];
        OsRng.fill_bytes(&mut salt_bytes);
        let raw_key = self._derive_key_with_salt(&salt_bytes)?;
        let cipher = Aes256Gcm::new_from_slice(&raw_key)
            .map_err(|e| ConfigDecryptionError::KdfError(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let encrypted = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
        let ct_len = encrypted.len() - 16;
        let ciphertext = &encrypted[..ct_len];
        let tag = &encrypted[ct_len..];
        let mut out = Vec::with_capacity(PBKDF2_SALT_LEN_V2 + 12 + 16 + ct_len);
        out.extend_from_slice(&salt_bytes);
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(tag);
        out.extend_from_slice(ciphertext);
        Ok(out)
    }

    /// Decrypt v2 wire bytes back to a UTF-8 string.
    ///
    /// Expected wire format: `salt[16] || nonce[12] || tag[16] || ciphertext`.
    pub(crate) fn _aes_decrypt_v2(&self, data: &[u8]) -> Result<String, ConfigDecryptionError> {
        if data.len() < MIN_WIRE_LEN_V2 {
            return Err(ConfigDecryptionError::AuthTagMismatch);
        }
        let salt = &data[..PBKDF2_SALT_LEN_V2];
        let rest = &data[PBKDF2_SALT_LEN_V2..];
        let raw_key = self._derive_key_with_salt(salt)?;
        let cipher = Aes256Gcm::new_from_slice(&raw_key)
            .map_err(|e| ConfigDecryptionError::KdfError(e.to_string()))?;
        let nonce = Nonce::from_slice(&rest[..12]);
        let tag = &rest[12..28];
        let ciphertext = &rest[28..];
        let mut ct_with_tag = Vec::with_capacity(ciphertext.len() + 16);
        ct_with_tag.extend_from_slice(ciphertext);
        ct_with_tag.extend_from_slice(tag);
        let plaintext = cipher
            .decrypt(nonce, ct_with_tag.as_slice())
            .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
        String::from_utf8(plaintext).map_err(|_| ConfigDecryptionError::InvalidUtf8)
    }

    /// Decrypt v1 (legacy) wire bytes back to a UTF-8 string.
    ///
    /// Expected wire format: `nonce[12] || tag[16] || ciphertext` with
    /// the static `PBKDF2_SALT_V1` salt.  Read-only — new encryptions
    /// use `_aes_encrypt_v2` / `enc:v2:` tokens instead.
    pub(crate) fn _aes_decrypt_v1(&self, data: &[u8]) -> Result<String, ConfigDecryptionError> {
        if data.len() < MIN_WIRE_LEN_V1 {
            return Err(ConfigDecryptionError::AuthTagMismatch);
        }
        let raw_key = self._derive_key_with_salt(PBKDF2_SALT_V1)?;
        let cipher = Aes256Gcm::new_from_slice(&raw_key)
            .map_err(|e| ConfigDecryptionError::KdfError(e.to_string()))?;
        let nonce = Nonce::from_slice(&data[..12]);
        let tag = &data[12..28];
        let ciphertext = &data[28..];
        let mut ct_with_tag = Vec::with_capacity(ciphertext.len() + 16);
        ct_with_tag.extend_from_slice(ciphertext);
        ct_with_tag.extend_from_slice(tag);
        let plaintext = cipher
            .decrypt(nonce, ct_with_tag.as_slice())
            .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
        String::from_utf8(plaintext).map_err(|_| ConfigDecryptionError::InvalidUtf8)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an encryptor that always uses the AES path (keyring skipped).
    fn aes_encryptor() -> ConfigEncryptor {
        ConfigEncryptor { _force_aes: true }
    }

    #[test]
    fn test_aes_v2_roundtrip() {
        let enc = aes_encryptor();
        let ciphertext = enc._aes_encrypt_v2("hello-secret").expect("encrypt");
        let plaintext = enc._aes_decrypt_v2(&ciphertext).expect("decrypt");
        assert_eq!(plaintext, "hello-secret");
    }

    #[test]
    fn test_store_without_keyring_returns_enc_v2_prefix() {
        let enc = aes_encryptor();
        let token = enc.store("auth.api_key", "secret123").expect("store");
        assert!(
            token.starts_with("enc:v2:"),
            "expected enc:v2: prefix, got {token}"
        );
    }

    #[test]
    fn test_retrieve_enc_v2_value() {
        let enc = aes_encryptor();
        let token = enc.store("auth.api_key", "secret123").expect("store");
        let result = enc.retrieve(&token, "auth.api_key").expect("retrieve");
        assert_eq!(result, "secret123");
    }

    #[test]
    fn test_retrieve_plaintext_passthrough() {
        let enc = aes_encryptor();
        let result = enc.retrieve("plain-value", "some.key").expect("retrieve");
        assert_eq!(result, "plain-value");
    }

    #[test]
    fn test_retrieve_corrupted_v1_ciphertext_returns_error() {
        let enc = aes_encryptor();
        let mut bad = vec![0u8; 40];
        bad[12] ^= 0xFF;
        let config_value = format!("enc:{}", B64.encode(&bad));
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_retrieve_corrupted_v2_ciphertext_returns_error() {
        let enc = aes_encryptor();
        // v2 wire: 16 salt + 40 (12 nonce + 16 tag + 12 ct), corrupt tag.
        let mut bad = vec![0u8; 56];
        bad[16 + 12] ^= 0xFF;
        let config_value = format!("enc:v2:{}", B64.encode(&bad));
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_retrieve_short_v1_ciphertext_returns_error() {
        let enc = aes_encryptor();
        let config_value = format!("enc:{}", B64.encode([0u8; 10]));
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_retrieve_short_v2_ciphertext_returns_error() {
        let enc = aes_encryptor();
        let config_value = format!("enc:v2:{}", B64.encode([0u8; 10]));
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_derive_key_is_32_bytes() {
        let enc = aes_encryptor();
        let key = enc._derive_key_with_salt(PBKDF2_SALT_V1).expect("derive");
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_v2_ciphertexts_differ_for_same_plaintext() {
        // Random per-encryption salt means same plaintext produces different tokens.
        let enc = aes_encryptor();
        let ct1 = enc._aes_encrypt_v2("same").expect("e1");
        let ct2 = enc._aes_encrypt_v2("same").expect("e2");
        assert_ne!(ct1, ct2, "v2 ciphertexts must differ (random salt)");
    }
}

// apcore-cli — Encrypted config storage.
// Protocol spec: SEC-03 (ConfigEncryptor, ConfigDecryptionError)

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
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
const PBKDF2_SALT: &[u8] = b"apcore-cli-config-v1";
const PBKDF2_ITERATIONS: u32 = 100_000;
/// Minimum wire-format length: 12-byte nonce + 16-byte tag.
const MIN_WIRE_LEN: usize = 28;

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
            let ciphertext = self._aes_encrypt(value)?;
            Ok(format!("enc:{}", B64.encode(&ciphertext)))
        }
    }

    /// Retrieve the plaintext for a config value token.
    ///
    /// Handles three formats:
    /// - `"keyring:<ref>"` — fetch from OS keyring.
    /// - `"enc:<base64>"` — base64-decode then AES-GCM decrypt.
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
        } else if let Some(b64_data) = config_value.strip_prefix("enc:") {
            let data = B64
                .decode(b64_data)
                .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
            self._aes_decrypt(&data).map_err(|e| match e {
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
    /// Material: `"<hostname>:<username>"`.
    /// Salt:     `b"apcore-cli-config-v1"`.
    /// Rounds:   100 000.
    ///
    /// **Design note:** The key material is intentionally low-entropy
    /// (hostname + username with a static salt) to match the Python reference
    /// implementation. This provides protection against casual file access but
    /// not against a targeted attacker who knows the hostname and username.
    /// For stronger protection, use the OS keyring path (`keyring:` prefix).
    fn _derive_key(&self) -> Result<[u8; 32], ConfigDecryptionError> {
        let hostname = gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        let material = format!("{hostname}:{username}");
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha256>(
            material.as_bytes(),
            PBKDF2_SALT,
            PBKDF2_ITERATIONS,
            &mut key,
        );
        Ok(key)
    }

    /// Encrypt `plaintext` and return the raw wire bytes.
    ///
    /// Wire format: `nonce[12] || tag[16] || ciphertext`.
    pub(crate) fn _aes_encrypt(&self, plaintext: &str) -> Result<Vec<u8>, ConfigDecryptionError> {
        let raw_key = self._derive_key()?;
        let cipher = Aes256Gcm::new_from_slice(&raw_key)
            .map_err(|e| ConfigDecryptionError::KdfError(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        // aes_gcm returns ciphertext || tag (tag is the last 16 bytes).
        let encrypted = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| ConfigDecryptionError::AuthTagMismatch)?;
        // Reorder to wire format: nonce || tag || ciphertext.
        let ct_len = encrypted.len() - 16;
        let ciphertext = &encrypted[..ct_len];
        let tag = &encrypted[ct_len..];
        let mut out = Vec::with_capacity(12 + 16 + ct_len);
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(tag);
        out.extend_from_slice(ciphertext);
        Ok(out)
    }

    /// Decrypt raw wire bytes back to a UTF-8 string.
    ///
    /// Expected wire format: `nonce[12] || tag[16] || ciphertext`.
    pub(crate) fn _aes_decrypt(&self, data: &[u8]) -> Result<String, ConfigDecryptionError> {
        if data.len() < MIN_WIRE_LEN {
            return Err(ConfigDecryptionError::AuthTagMismatch);
        }
        let raw_key = self._derive_key()?;
        let cipher = Aes256Gcm::new_from_slice(&raw_key)
            .map_err(|e| ConfigDecryptionError::KdfError(e.to_string()))?;
        let nonce = Nonce::from_slice(&data[..12]);
        let tag = &data[12..28];
        let ciphertext = &data[28..];
        // aes_gcm::decrypt expects ciphertext || tag.
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
    fn test_aes_roundtrip() {
        // Encrypt then decrypt must recover the original plaintext.
        let enc = aes_encryptor();
        let ciphertext = enc._aes_encrypt("hello-secret").expect("encrypt");
        let plaintext = enc._aes_decrypt(&ciphertext).expect("decrypt");
        assert_eq!(plaintext, "hello-secret");
    }

    #[test]
    fn test_store_without_keyring_returns_enc_prefix() {
        let enc = aes_encryptor();
        let token = enc.store("auth.api_key", "secret123").expect("store");
        assert!(
            token.starts_with("enc:"),
            "expected enc: prefix, got {token}"
        );
    }

    #[test]
    fn test_retrieve_enc_value() {
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
    fn test_retrieve_corrupted_ciphertext_returns_error() {
        let enc = aes_encryptor();
        // 28 bytes minimum: 12 nonce + 16 tag; pad with zeroes then corrupt tag.
        let mut bad = vec![0u8; 40];
        bad[12] ^= 0xFF; // corrupt tag byte
        let b64 = B64.encode(&bad);
        let config_value = format!("enc:{b64}");
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_retrieve_short_ciphertext_returns_error() {
        let enc = aes_encryptor();
        // Fewer than 28 bytes — missing nonce+tag.
        let b64 = B64.encode([0u8; 10]);
        let config_value = format!("enc:{b64}");
        let result = enc.retrieve(&config_value, "some.key");
        assert!(matches!(
            result,
            Err(ConfigDecryptionError::AuthTagMismatch)
        ));
    }

    #[test]
    fn test_derive_key_is_32_bytes() {
        let enc = aes_encryptor();
        let key = enc._derive_key().expect("derive");
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_nonces_are_unique() {
        // Each encrypt call must produce a different nonce (probabilistically).
        let enc = aes_encryptor();
        let ct1 = enc._aes_encrypt("same").expect("e1");
        let ct2 = enc._aes_encrypt("same").expect("e2");
        assert_ne!(&ct1[..12], &ct2[..12], "nonces must differ");
    }
}

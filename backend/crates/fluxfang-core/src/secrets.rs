//! Secret encryption helper for alert-method credentials (SMTP passwords,
//! webhook secrets) stored at rest in `alert_method.config_encrypted`
//! (Task 8.1). Pure functions only — no I/O, no env reading; the caller
//! (API layer, Task 6.6) is responsible for loading `FLUXFANG_SECRET_KEY`
//! and passing the raw key bytes in.
//!
//! Uses AES-256-GCM with a random 96-bit (12-byte) nonce per encryption.
//! The nonce is prepended to the ciphertext (which includes the GCM tag):
//! `output = nonce (12 bytes) || ciphertext || tag`.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::fmt;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Error returned by [`decrypt`] and [`key_from_base64`].
///
/// Deliberately low-detail (no ciphertext/key material, no distinction
/// between "wrong key" and "tampered data") so it's safe to log or bubble
/// up without leaking anything about the secret being protected.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SecretError {
    /// `decrypt` input was shorter than the 12-byte nonce prefix.
    InputTooShort,
    /// AES-GCM authentication failed: tampered/corrupt ciphertext, tag, or
    /// nonce, or the wrong key was used.
    DecryptionFailed,
    /// `key_from_base64` input wasn't valid base64.
    InvalidBase64,
    /// `key_from_base64` input decoded to something other than 32 bytes.
    InvalidKeyLength,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            SecretError::InputTooShort => "encrypted data is too short to contain a nonce",
            SecretError::DecryptionFailed => "decryption failed: authentication check failed",
            SecretError::InvalidBase64 => "secret key is not valid base64",
            SecretError::InvalidKeyLength => "secret key must decode to exactly 32 bytes",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for SecretError {}

/// Encrypt `plaintext` with AES-256-GCM under `key`.
///
/// Generates a fresh random 12-byte nonce (via [`OsRng`]) for every call and
/// prepends it to the output: `nonce (12) || ciphertext || tag`. Pass the
/// full output straight to [`decrypt`].
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    // Encryption with a freshly generated, correctly-sized key/nonce pair
    // cannot fail per the `aead` crate's contract.
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("AES-256-GCM encryption with a valid key/nonce should not fail");

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt data produced by [`encrypt`] under the same `key`.
///
/// Returns `Err(SecretError::InputTooShort)` if `data` is shorter than the
/// 12-byte nonce prefix, or `Err(SecretError::DecryptionFailed)` if
/// authentication fails (tampered ciphertext/tag/nonce, or wrong key).
/// Never panics on malformed input.
pub fn decrypt(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, SecretError> {
    if data.len() < NONCE_LEN {
        return Err(SecretError::InputTooShort);
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(nonce_bytes.into(), ciphertext)
        .map_err(|_| SecretError::DecryptionFailed)
}

/// Parse `FLUXFANG_SECRET_KEY`-style input: a base64-encoded 32-byte key.
///
/// Returns `Err(SecretError::InvalidBase64)` if `s` isn't valid base64, or
/// `Err(SecretError::InvalidKeyLength)` if it decodes to anything other
/// than exactly 32 bytes.
pub fn key_from_base64(s: &str) -> Result<[u8; KEY_LEN], SecretError> {
    let bytes = BASE64
        .decode(s.trim())
        .map_err(|_| SecretError::InvalidBase64)?;
    bytes.try_into().map_err(|_| SecretError::InvalidKeyLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_LEN] {
        [0x42u8; KEY_LEN]
    }

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let key = test_key();
        let plaintext = b"super-secret-smtp-password";
        let ciphertext = encrypt(&key, plaintext);
        let decrypted = decrypt(&key, &ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn tampered_ciphertext_fails_to_decrypt() {
        let key = test_key();
        let plaintext = b"webhook-secret-token";
        let mut ciphertext = encrypt(&key, plaintext);
        // Flip a byte well past the nonce, inside the actual ciphertext/tag.
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        let result = decrypt(&key, &ciphertext);
        assert_eq!(result, Err(SecretError::DecryptionFailed));
    }

    #[test]
    fn tampered_nonce_fails_to_decrypt() {
        let key = test_key();
        let plaintext = b"another-secret";
        let mut ciphertext = encrypt(&key, plaintext);
        ciphertext[0] ^= 0xFF;
        assert_eq!(
            decrypt(&key, &ciphertext),
            Err(SecretError::DecryptionFailed)
        );
    }

    #[test]
    fn two_encryptions_of_same_plaintext_differ() {
        let key = test_key();
        let plaintext = b"same-plaintext-every-time";
        let a = encrypt(&key, plaintext);
        let b = encrypt(&key, plaintext);
        assert_ne!(a, b, "random nonce should make ciphertexts differ");
        // Both must still independently decrypt to the same plaintext.
        assert_eq!(decrypt(&key, &a).unwrap(), plaintext);
        assert_eq!(decrypt(&key, &b).unwrap(), plaintext);
    }

    #[test]
    fn too_short_input_returns_err_not_panic() {
        let key = test_key();
        assert_eq!(decrypt(&key, b"").unwrap_err(), SecretError::InputTooShort);
        assert_eq!(
            decrypt(&key, b"short").unwrap_err(),
            SecretError::InputTooShort
        );
        // Exactly 11 bytes (one short of the nonce length).
        assert_eq!(
            decrypt(&key, &[0u8; 11]).unwrap_err(),
            SecretError::InputTooShort
        );
    }

    #[test]
    fn nonce_only_input_fails_decryption_not_panics() {
        let key = test_key();
        // Exactly 12 bytes: passes the length check but has no ciphertext/tag.
        let result = decrypt(&key, &[0u8; NONCE_LEN]);
        assert!(result.is_err());
    }

    #[test]
    fn key_from_base64_accepts_valid_32_byte_key() {
        let raw = [7u8; KEY_LEN];
        let encoded = BASE64.encode(raw);
        let parsed = key_from_base64(&encoded).expect("should parse valid key");
        assert_eq!(parsed, raw);
    }

    #[test]
    fn key_from_base64_rejects_wrong_length() {
        let too_short = BASE64.encode([1u8; 16]);
        assert_eq!(
            key_from_base64(&too_short).unwrap_err(),
            SecretError::InvalidKeyLength
        );

        let too_long = BASE64.encode([1u8; 33]);
        assert_eq!(
            key_from_base64(&too_long).unwrap_err(),
            SecretError::InvalidKeyLength
        );
    }

    #[test]
    fn key_from_base64_rejects_invalid_base64() {
        assert_eq!(
            key_from_base64("not valid base64!!!").unwrap_err(),
            SecretError::InvalidBase64
        );
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key = test_key();
        let wrong_key = [0x99u8; KEY_LEN];
        let ciphertext = encrypt(&key, b"data");
        assert_eq!(
            decrypt(&wrong_key, &ciphertext),
            Err(SecretError::DecryptionFailed)
        );
    }
}

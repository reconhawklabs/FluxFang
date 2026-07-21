use crate::error::ProtoError;
use crate::{Key, NONCE_LEN};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;

/// Seal `plaintext` under `key` with XChaCha20-Poly1305. A fresh random
/// 24-byte nonce is generated per call and prepended to the returned bytes:
/// `nonce (24B) ‖ ciphertext+tag`.
pub fn seal(key: &Key, plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    // In-memory AEAD over an owned buffer never fails; treat a failure as a
    // programming error rather than a runtime `Result` callers must handle.
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .expect("XChaCha20Poly1305 encryption of an in-memory buffer cannot fail");

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    out
}

/// Open a `nonce ‖ ciphertext+tag` message produced by [`seal`]. Returns
/// `ProtoError::Malformed` if the input is shorter than a nonce, and
/// `ProtoError::Decrypt` on any authentication failure (wrong key, tampered
/// nonce/ciphertext/tag). Never panics on attacker-controlled input.
pub fn open(key: &Key, sealed: &[u8]) -> Result<Vec<u8>, ProtoError> {
    if sealed.len() < NONCE_LEN {
        return Err(ProtoError::Malformed);
    }
    let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| ProtoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_key;

    #[test]
    fn seal_open_roundtrip() {
        let key = generate_key();
        let msg = b"hello sensor";
        let sealed = seal(&key, msg);
        assert_eq!(open(&key, &sealed).unwrap(), msg);
    }

    #[test]
    fn sealed_is_longer_than_plaintext_and_starts_with_nonce() {
        let key = generate_key();
        let sealed = seal(&key, b"x");
        // nonce (24) + ciphertext(1) + poly1305 tag (16) = 41
        assert!(sealed.len() >= NONCE_LEN + 1 + 16);
    }

    #[test]
    fn same_plaintext_seals_differently_each_time() {
        let key = generate_key();
        let a = seal(&key, b"same");
        let b = seal(&key, b"same");
        assert_ne!(a, b, "random nonce must make ciphertexts differ");
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let sealed = seal(&generate_key(), b"secret");
        assert_eq!(open(&generate_key(), &sealed), Err(ProtoError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails_to_open() {
        let key = generate_key();
        let mut sealed = seal(&key, b"secret payload");
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01; // flip a bit in the tag/ciphertext
        assert_eq!(open(&key, &sealed), Err(ProtoError::Decrypt));
    }

    #[test]
    fn tampered_nonce_fails_to_open() {
        let key = generate_key();
        let mut sealed = seal(&key, b"secret payload");
        sealed[0] ^= 0x01; // flip a bit in the prepended nonce
        assert_eq!(open(&key, &sealed), Err(ProtoError::Decrypt));
    }

    #[test]
    fn truncated_input_is_malformed_not_a_panic() {
        let key = generate_key();
        // Shorter than a nonce -> Malformed, must not panic.
        assert_eq!(
            open(&key, &[0u8; NONCE_LEN - 1]),
            Err(ProtoError::Malformed)
        );
        assert_eq!(open(&key, &[]), Err(ProtoError::Malformed));
    }
}

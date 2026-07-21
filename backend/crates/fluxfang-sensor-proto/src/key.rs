use crate::error::ProtoError;
use crate::{Key, KEY_LEN};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;

/// Generate a fresh 32-byte key from the OS CSPRNG.
pub fn generate_key() -> Key {
    let mut key = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);
    key
}

/// Internal: base64-encode arbitrary bytes (standard alphabet). Used by
/// [`encode_key`] and, in tests, to build wrong-length inputs.
pub(crate) fn encode_bytes(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Base64-encode a key (standard alphabet).
pub fn encode_key(key: &Key) -> String {
    encode_bytes(key)
}

/// Decode a base64 key. Returns `ProtoError::Base64` on invalid base64 and
/// `ProtoError::KeyLength` when the decoded byte length is not `KEY_LEN`.
pub fn decode_key(s: &str) -> Result<Key, ProtoError> {
    let bytes = STANDARD.decode(s).map_err(|_| ProtoError::Base64)?;
    let arr: Key = bytes.try_into().map_err(|_| ProtoError::KeyLength)?;
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_key_is_32_bytes_and_not_all_zero() {
        let k = generate_key();
        assert_eq!(k.len(), KEY_LEN);
        assert!(k.iter().any(|&b| b != 0), "key should not be all zeros");
    }

    #[test]
    fn two_generated_keys_differ() {
        assert_ne!(generate_key(), generate_key());
    }

    #[test]
    fn encode_then_decode_roundtrips() {
        let k = generate_key();
        let encoded = encode_key(&k);
        let decoded = decode_key(&encoded).expect("roundtrip decode");
        assert_eq!(decoded, k);
    }

    #[test]
    fn decode_rejects_wrong_length() {
        // base64 of 3 bytes -> valid base64, wrong length.
        let short = crate::key::encode_bytes(&[1u8, 2, 3]);
        assert_eq!(decode_key(&short), Err(ProtoError::KeyLength));
    }

    #[test]
    fn decode_rejects_bad_base64() {
        assert_eq!(decode_key("not valid base64!!!"), Err(ProtoError::Base64));
    }
}

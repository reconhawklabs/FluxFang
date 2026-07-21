use std::fmt;

/// Errors returned by the wire-protocol primitives. Deliberately coarse:
/// AEAD failures collapse to `Decrypt` so callers can't distinguish a wrong
/// key from a tampered ciphertext (both mean "reject").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// AEAD open failed — wrong key or tampered ciphertext/nonce/tag.
    Decrypt,
    /// Framing was malformed (e.g. input shorter than the nonce).
    Malformed,
    /// JSON (de)serialization of the batch envelope failed.
    Json(String),
    /// A decoded key was not exactly `KEY_LEN` bytes.
    KeyLength,
    /// Base64 decoding failed.
    Base64,
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Decrypt => write!(f, "AEAD open failed (wrong key or tampered data)"),
            ProtoError::Malformed => write!(f, "malformed sealed message"),
            ProtoError::Json(e) => write!(f, "batch json error: {e}"),
            ProtoError::KeyLength => write!(f, "key must be 32 bytes"),
            ProtoError::Base64 => write!(f, "invalid base64"),
        }
    }
}

impl std::error::Error for ProtoError {}

//! `fluxfang-sensor-proto`: the Sensor↔Standalone wire protocol primitives.
//!
//! Pure, I/O-free. Provides per-sensor symmetric AEAD (XChaCha20-Poly1305),
//! a JSON batch envelope, a replay-window check, and a human-verifiable key
//! fingerprint. Consumed by the Standalone listener (enrollment/ingest) and
//! the Sensor forwarder in later phases; nothing here does networking.

// mod batch; // added in Task 2/3
mod error;
// mod fingerprint; // added in Task 2/3
mod key;
// mod seal; // added in Task 2/3

/// Symmetric key length in bytes (XChaCha20-Poly1305 uses a 256-bit key).
pub const KEY_LEN: usize = 32;
/// Nonce length in bytes (XChaCha20 uses a 192-bit nonce).
pub const NONCE_LEN: usize = 24;

/// A 32-byte symmetric key. Opaque bytes at this layer; base64 at the
/// config/API boundary (see [`encode_key`]/[`decode_key`]).
pub type Key = [u8; KEY_LEN];

// pub use batch::{open_batch, seal_batch, within_replay_window, SensorBatch, WireEmission}; // added in Task 2/3
pub use error::ProtoError;
// pub use fingerprint::fingerprint; // added in Task 2/3
pub use key::{decode_key, encode_key, generate_key};
// pub use seal::{open, seal}; // added in Task 2/3

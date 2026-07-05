//! Password hashing helper, used by the first-run setup + login flow (Task
//! 2.2). Pure functions only — no I/O, no DB. Hashes are argon2 PHC-format
//! strings (self-describing: algorithm, params, salt, and digest all live in
//! the one string), suitable for storing directly in `app_config.password_hash`.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand_core::OsRng;

/// Hash `password` with Argon2 (default params) and a fresh random salt.
///
/// Returns a self-describing PHC-format string (e.g.
/// `$argon2id$v=19$m=19456,t=2,p=1$...$...`) that [`verify_password`] can
/// later check against.
pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hashing with a freshly generated salt should not fail")
        .to_string()
}

/// Check whether `candidate` matches the password that produced `hash`.
///
/// Returns `false` (never panics) both on a genuine mismatch and on a
/// malformed/garbage `hash` string.
pub fn verify_password(hash: &str, candidate: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(candidate.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrips() {
        let h = hash_password("hunter2");
        assert!(verify_password(&h, "hunter2"));
        assert!(!verify_password(&h, "wrong"));
    }

    #[test]
    fn same_password_hashed_twice_yields_different_hashes() {
        let h1 = hash_password("hunter2");
        let h2 = hash_password("hunter2");
        assert_ne!(h1, h2, "salts should be random per hash");
        // Both must still independently verify.
        assert!(verify_password(&h1, "hunter2"));
        assert!(verify_password(&h2, "hunter2"));
    }

    #[test]
    fn verify_password_on_malformed_hash_returns_false() {
        assert!(!verify_password("not-a-valid-phc-hash", "hunter2"));
        assert!(!verify_password("", "hunter2"));
        assert!(!verify_password("argon2$fake-hash-1", "hunter2"));
    }

    #[test]
    fn empty_password_roundtrips() {
        let h = hash_password("");
        assert!(verify_password(&h, ""));
        assert!(!verify_password(&h, "not-empty"));
    }
}

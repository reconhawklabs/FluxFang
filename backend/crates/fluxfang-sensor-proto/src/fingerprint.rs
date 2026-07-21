use crate::Key;
use sha2::{Digest, Sha256};

/// A short, human-verifiable fingerprint of a sensor's `(sensor_id, key)`
/// pair: `SHA-256(sensor_id_bytes ‖ key)`, first 4 bytes, uppercase hex in
/// dash-separated byte groups (e.g. `4F-A2-09-EE`). Shown on both the
/// Standalone approval screen and the Sensor node so an operator can confirm
/// they match out-of-band before approving.
pub fn fingerprint(sensor_id: &str, key: &Key) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sensor_id.as_bytes());
    hasher.update(key);
    let digest = hasher.finalize();
    digest[..4]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_key;

    #[test]
    fn fingerprint_matches_format() {
        let fp = fingerprint("frontgate", &generate_key());
        let re_ok = fp.len() == 11
            && fp.split('-').count() == 4
            && fp.split('-').all(|g| {
                g.len() == 2
                    && g.chars().all(|c| {
                        c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_uppercase())
                    })
            });
        assert!(re_ok, "unexpected fingerprint format: {fp}");
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let key = generate_key();
        assert_eq!(
            fingerprint("frontgate", &key),
            fingerprint("frontgate", &key)
        );
    }

    #[test]
    fn fingerprint_differs_on_different_key() {
        assert_ne!(
            fingerprint("frontgate", &generate_key()),
            fingerprint("frontgate", &generate_key())
        );
    }

    #[test]
    fn fingerprint_differs_on_different_sensor_id() {
        let key = generate_key();
        assert_ne!(fingerprint("frontgate", &key), fingerprint("backlot", &key));
    }
}

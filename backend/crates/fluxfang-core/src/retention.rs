//! Per-data-source MAC retention settings: how persistent an address has to
//! be before an emission is worth storing, and whether short-lived emitters
//! should be aged back out.
//!
//! Both settings live in the data source's existing `config` JSONB (no
//! schema change) under `mac_retention_level` and `age_out_ephemeral`, and
//! both only apply to the kinds that actually see randomized addresses --
//! see [`supports_mac_retention`].
//!
//! The defaults are deliberately inert: an absent `mac_retention_level`
//! means "store everything", so existing data sources keep behaving exactly
//! as they did before this module existed. Dropping capture data is only
//! ever something the operator opts into.

use crate::classify::MacPersistence;
use serde_json::Value;

/// Config key holding the retention level (a [`MacPersistence`] token, or
/// absent/null for "store everything").
pub const LEVEL_KEY: &str = "mac_retention_level";
/// Config key holding the age-out-ephemeral-emitters flag.
pub const AGE_OUT_KEY: &str = "age_out_ephemeral";

/// How long an ephemeral-class emitter may go unseen before the age-out
/// sweep removes it. Fixed rather than configurable: the point of the class
/// is that its addresses rotate on the order of minutes, so an hour is
/// already generous.
pub const AGE_OUT_AFTER_SECS: i64 = 3600;

/// A data source's resolved MAC retention settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MacRetention {
    /// Least-persistent class still stored. `None` means store everything,
    /// which is the default for a source that has never been configured.
    pub level: Option<MacPersistence>,
    /// Whether ephemeral-class emitters from this source are deleted once
    /// they've gone unseen for [`AGE_OUT_AFTER_SECS`].
    pub age_out_ephemeral: bool,
}

impl MacRetention {
    /// Whether an emission whose address falls in `class` should be stored.
    ///
    /// `None` -- an emitter type with no address at all, e.g. a TPMS sensor
    /// id -- is always stored: there is no randomization to filter, so a
    /// retention level must not silently discard it.
    pub fn should_store(&self, class: Option<MacPersistence>) -> bool {
        match (self.level, class) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(level), Some(class)) => class.retained_at(level),
        }
    }
}

/// Whether `kind` has addresses that can be randomized, and so should be
/// offered the retention settings at all. Wi-Fi and Bluetooth do; GPS has
/// no addresses and TPMS sensor ids are never randomized.
pub fn supports_mac_retention(kind: &str) -> bool {
    matches!(kind, "wifi" | "bluetooth")
}

/// Read the retention settings out of a data source's `config`. Absent or
/// malformed values fall back to the inert defaults rather than erroring --
/// [`validate_config`] is where bad input is rejected, at the API boundary;
/// by the time a row is being read it has already been validated, and a
/// hand-edited row should degrade to "store everything" rather than start
/// dropping captures.
pub fn from_config(config: &Value) -> MacRetention {
    MacRetention {
        level: config
            .get(LEVEL_KEY)
            .and_then(Value::as_str)
            .and_then(MacPersistence::parse),
        age_out_ephemeral: config
            .get(AGE_OUT_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// Validate the retention keys in a data source `config` for a source of
/// `kind`. Returns a human-readable message the API surfaces as a 400.
///
/// A `null` level is accepted as an explicit "store everything" so the
/// frontend can clear the dropdown without having to delete the key.
/// Setting either key on a kind that has no randomized addresses is an
/// error rather than a silent no-op -- it would otherwise look like the
/// operator had limited retention when nothing was being filtered.
pub fn validate_config(kind: &str, config: &Value) -> Result<(), String> {
    let has_level = config.get(LEVEL_KEY).is_some_and(|v| !v.is_null());
    let has_age_out = config.get(AGE_OUT_KEY).is_some_and(|v| !v.is_null());

    if !supports_mac_retention(kind) {
        if has_level || has_age_out {
            return Err(format!(
                "'{LEVEL_KEY}'/'{AGE_OUT_KEY}' only apply to wifi and bluetooth data sources, not '{kind}'"
            ));
        }
        return Ok(());
    }

    if let Some(v) = config.get(LEVEL_KEY) {
        if !v.is_null() {
            let token = v.as_str().ok_or_else(|| {
                format!("'{LEVEL_KEY}' must be a string, or null to store everything")
            })?;
            if MacPersistence::parse(token).is_none() {
                let allowed: Vec<&str> = MacPersistence::ALL.iter().map(|c| c.as_str()).collect();
                return Err(format!(
                    "'{LEVEL_KEY}' must be one of {allowed:?}, or null to store everything; got '{token}'"
                ));
            }
        }
    }

    if let Some(v) = config.get(AGE_OUT_KEY) {
        if !v.is_null() && !v.is_boolean() {
            return Err(format!("'{AGE_OUT_KEY}' must be a boolean"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_config_stores_everything_and_does_not_age_out() {
        let r = from_config(&json!({}));
        assert_eq!(r, MacRetention::default());
        assert_eq!(r.level, None);
        assert!(!r.age_out_ephemeral);
        for c in MacPersistence::ALL {
            assert!(r.should_store(Some(c)), "{c:?} must be stored by default");
        }
    }

    #[test]
    fn session_level_stores_session_and_above_only() {
        let r = from_config(&json!({ LEVEL_KEY: "session" }));
        assert_eq!(r.level, Some(MacPersistence::Session));
        assert!(r.should_store(Some(MacPersistence::Stable)));
        assert!(r.should_store(Some(MacPersistence::PerNetwork)));
        assert!(r.should_store(Some(MacPersistence::Session)));
        assert!(!r.should_store(Some(MacPersistence::Ephemeral)));
        assert!(!r.should_store(Some(MacPersistence::Unlinkable)));
    }

    #[test]
    fn classless_emitters_are_always_stored_whatever_the_level() {
        for level in MacPersistence::ALL {
            let r = MacRetention {
                level: Some(level),
                age_out_ephemeral: false,
            };
            assert!(
                r.should_store(None),
                "a TPMS sensor id must survive level {level:?}"
            );
        }
    }

    #[test]
    fn malformed_config_degrades_to_storing_everything() {
        for bad in [
            json!({ LEVEL_KEY: "not-a-class" }),
            json!({ LEVEL_KEY: 7 }),
            json!({ LEVEL_KEY: null }),
        ] {
            assert_eq!(from_config(&bad).level, None, "config {bad}");
        }
        assert!(!from_config(&json!({ AGE_OUT_KEY: "yes" })).age_out_ephemeral);
    }

    #[test]
    fn age_out_flag_round_trips() {
        assert!(from_config(&json!({ AGE_OUT_KEY: true })).age_out_ephemeral);
        assert!(!from_config(&json!({ AGE_OUT_KEY: false })).age_out_ephemeral);
    }

    #[test]
    fn validate_accepts_every_class_plus_null_and_absent() {
        for c in MacPersistence::ALL {
            assert!(validate_config("wifi", &json!({ LEVEL_KEY: c.as_str() })).is_ok());
        }
        assert!(validate_config("bluetooth", &json!({ LEVEL_KEY: null })).is_ok());
        assert!(validate_config("wifi", &json!({})).is_ok());
        assert!(validate_config("wifi", &json!({ AGE_OUT_KEY: true })).is_ok());
    }

    #[test]
    fn validate_rejects_unknown_level_and_non_bool_age_out() {
        let err = validate_config("wifi", &json!({ LEVEL_KEY: "randomized" })).unwrap_err();
        assert!(
            err.contains("randomized"),
            "message should echo input: {err}"
        );
        assert!(validate_config("wifi", &json!({ LEVEL_KEY: 3 })).is_err());
        assert!(validate_config("wifi", &json!({ AGE_OUT_KEY: "true" })).is_err());
    }

    #[test]
    fn validate_rejects_retention_keys_on_kinds_without_randomized_addresses() {
        assert!(validate_config("gps", &json!({ LEVEL_KEY: "session" })).is_err());
        assert!(validate_config("tpms", &json!({ AGE_OUT_KEY: true })).is_err());
        // ...but leaves those kinds alone when the keys aren't set.
        assert!(validate_config("gps", &json!({ "host": "localhost" })).is_ok());
        assert!(validate_config("gps", &json!({ LEVEL_KEY: null })).is_ok());
    }

    #[test]
    fn supports_mac_retention_covers_exactly_wifi_and_bluetooth() {
        assert!(supports_mac_retention("wifi"));
        assert!(supports_mac_retention("bluetooth"));
        assert!(!supports_mac_retention("gps"));
        assert!(!supports_mac_retention("tpms"));
        assert!(!supports_mac_retention("sensor"));
    }
}

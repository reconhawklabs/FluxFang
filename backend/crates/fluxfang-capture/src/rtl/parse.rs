//! Pure `rtl_433 -F json` line → [`RawObservation`] mapping for TPMS.
//!
//! No I/O and no clock reads: the caller (the capturer thread) samples the
//! wall clock once per line and passes it as `fallback_time`, keeping this
//! function deterministic and unit-testable — the same boundary
//! `gps::serial` documents.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::RawObservation;

/// Parse one line of `rtl_433 -F json` output into a TPMS [`RawObservation`],
/// or `None` if the line isn't JSON, isn't a `type == "TPMS"` record, or
/// lacks the `id` needed for a stable identity. `fallback_time` becomes the
/// observation's `observed_at` (rtl_433's own `time` string is preserved in
/// the payload; on a live stream the two are within milliseconds, and using
/// the capture clock avoids rtl_433's local-time-without-offset ambiguity).
pub fn parse_tpms_line(line: &str, fallback_time: DateTime<Utc>) -> Option<RawObservation> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("TPMS") {
        return None;
    }
    let id = normalize_id(v.get("id")?)?;

    let mut payload = json!({ "id": id, "type": "TPMS" });
    // Copy through the fields we log, preserving each value's JSON type.
    for key in [
        "model",
        "status",
        "pressure_PSI",
        "temperature_C",
        "rssi",
        "snr",
        "noise",
        "time",
    ] {
        if let Some(val) = v.get(key) {
            payload[key] = val.clone();
        }
    }
    // rtl_433 reports the tuned frequency as `freq1` (MHz); expose it as `freq`.
    if let Some(freq) = v.get("freq1") {
        payload["freq"] = freq.clone();
    }

    let signal_strength = v
        .get("rssi")
        .and_then(Value::as_f64)
        .map(|r| r.round() as i32);

    Some(RawObservation {
        kind: "tpms".to_string(),
        observed_at: fallback_time,
        signal_strength,
        payload,
    })
}

/// Normalize the `id` field to a lowercase string. rtl_433 emits `id` as a
/// JSON string for some decoders (e.g. hex `"d8af50f2"`) and as a number for
/// others; either way we key the emitter's identity on a consistent string.
/// Returns `None` for an empty/absent id.
fn normalize_id(id: &Value) -> Option<String> {
    let s = match id {
        Value::String(s) => s.to_ascii_lowercase(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn base() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 7, 21, 47, 19).unwrap()
    }

    const TOYOTA_LINE: &str = r#"{"time":"2026-07-07 21:47:19","model":"Toyota","type":"TPMS","id":"d8af50f2","status":128,"pressure_PSI":31.000,"temperature_C":24.000,"mic":"CRC","mod":"FSK","freq1":315.0,"freq2":315.0,"rssi":1.0,"snr":17.1,"noise":-17.1}"#;

    #[test]
    fn parses_a_toyota_tpms_record() {
        let obs = parse_tpms_line(TOYOTA_LINE, base()).expect("valid TPMS line parses");
        assert_eq!(obs.kind, "tpms");
        assert_eq!(obs.observed_at, base());
        assert_eq!(obs.signal_strength, Some(1)); // rssi 1.0 rounded
        assert_eq!(obs.payload["id"], "d8af50f2");
        assert_eq!(obs.payload["model"], "Toyota");
        assert_eq!(obs.payload["type"], "TPMS");
        assert_eq!(obs.payload["status"], 128);
        assert_eq!(obs.payload["pressure_PSI"], 31.0);
        assert_eq!(obs.payload["temperature_C"], 24.0);
        assert_eq!(obs.payload["snr"], 17.1);
        assert_eq!(obs.payload["time"], "2026-07-07 21:47:19");
    }

    #[test]
    fn numeric_id_is_normalized_to_string() {
        let line = r#"{"model":"Ford","type":"TPMS","id":123456,"pressure_PSI":32.0}"#;
        let obs = parse_tpms_line(line, base()).unwrap();
        assert_eq!(obs.payload["id"], "123456");
    }

    #[test]
    fn missing_rssi_yields_none_signal_strength() {
        let line = r#"{"model":"Ford","type":"TPMS","id":"ab12","pressure_PSI":32.0}"#;
        let obs = parse_tpms_line(line, base()).unwrap();
        assert_eq!(obs.signal_strength, None);
    }

    #[test]
    fn non_tpms_record_is_ignored() {
        let line = r#"{"model":"Acurite-Tower","type":"TH","id":"abcd","temperature_C":20.0}"#;
        assert!(parse_tpms_line(line, base()).is_none());
    }

    #[test]
    fn missing_id_is_none() {
        let line = r#"{"model":"Toyota","type":"TPMS","pressure_PSI":31.0}"#;
        assert!(parse_tpms_line(line, base()).is_none());
    }

    #[test]
    fn banner_and_blank_lines_are_none() {
        assert!(parse_tpms_line("", base()).is_none());
        assert!(parse_tpms_line("rtl_433 version 25.12", base()).is_none());
        assert!(parse_tpms_line("[SDR] Using device 0", base()).is_none());
    }
}

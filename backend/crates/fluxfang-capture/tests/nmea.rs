//! Tests `parse_nmea` against $GPGGA/$GPRMC sentences - no serial hardware
//! involved. See `src/gps/nmea.rs` for the pure parser and its unit
//! conventions (units, date handling, hemisphere sign).

use chrono::NaiveDate;
use fluxfang_capture::gps::parse_nmea;

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 5).unwrap()
}

#[test]
fn parses_gpgga_fix() {
    let line = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    let fix = parse_nmea(line, date()).unwrap();
    assert!((fix.lat - 48.1173).abs() < 0.001);
    assert!((fix.lon - 11.5167).abs() < 0.001);
    assert_eq!(fix.quality, 1);
    assert!((fix.altitude.unwrap() - 545.4).abs() < 0.001);
    // GGA carries no date; the caller-supplied `date` combines with the
    // sentence's time-of-day (12:35:19 UTC) to stamp `at`.
    assert_eq!(fix.at.date_naive(), date());
    assert_eq!(
        fix.at.format("%H:%M:%S").to_string(),
        "12:35:19".to_string()
    );
}

#[test]
fn parses_gprmc_fix_with_speed_and_heading() {
    let line = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
    let fix = parse_nmea(line, date()).unwrap();
    assert!((fix.lat - 48.1173).abs() < 0.001);
    assert!((fix.lon - 11.5167).abs() < 0.001);
    // 022.4 knots -> m/s (knots * 0.514444).
    assert!((fix.speed.unwrap() - 022.4_f64 * 0.514444).abs() < 0.001);
    assert!((fix.heading.unwrap() - 84.4).abs() < 0.001);
}

#[test]
fn hemisphere_south_and_west_yield_negative_degrees() {
    let line = "$GPGGA,123519,4807.038,S,01131.000,W,1,08,0.9,545.4,M,46.9,M,,*74";
    let fix = parse_nmea(line, date()).unwrap();
    assert!(fix.lat < 0.0);
    assert!(fix.lon < 0.0);
}

#[test]
fn gga_quality_zero_is_no_fix() {
    let line = "$GPGGA,123519,4807.038,N,01131.000,E,0,08,0.9,545.4,M,46.9,M,,*46";
    assert_eq!(parse_nmea(line, date()), None);
}

#[test]
fn rmc_status_void_is_no_fix() {
    let line = "$GPRMC,123519,V,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6B";
    assert_eq!(parse_nmea(line, date()), None);
}

#[test]
fn malformed_short_line_returns_none_without_panic() {
    assert_eq!(parse_nmea("$GPGGA,123519,*00", date()), None);
    assert_eq!(parse_nmea("", date()), None);
    assert_eq!(parse_nmea("garbage", date()), None);
}

#[test]
fn unrecognized_sentence_returns_none() {
    let line = "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39";
    assert_eq!(parse_nmea(line, date()), None);
}

#[test]
fn gga_non_numeric_quality_field_returns_none() {
    // Correct field count, but the quality slot (index 6) holds a
    // non-numeric value. `.parse().ok()?` should reject it cleanly.
    let line = "$GPGGA,123519,4807.038,N,01131.000,E,X,08,0.9,545.4,M,46.9,M,,*00";
    assert_eq!(parse_nmea(line, date()), None);
}

#[test]
fn gga_multibyte_char_in_lat_field_returns_none_without_panic() {
    // Regression test: the lat field is "0é31.000" - a multi-byte UTF-8
    // character ('é', bytes 1..3) straddling byte offset 2, which is
    // where `parse_coord` used to call `raw.split_at(2)` for a lat field
    // (degree_digits == 2). `str::split_at` panics if the offset isn't on
    // a UTF-8 char boundary; byte offset 2 falls in the *middle* of 'é's
    // 2-byte encoding (confirmed via `str::is_char_boundary`), so this
    // used to kill the whole process instead of returning `None`. Field
    // count is otherwise well-formed (10 GGA fields); only the lat field
    // is garbled.
    let line = "$GPGGA,123519,0\u{e9}31.000,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    assert_eq!(parse_nmea(line, date()), None);
}

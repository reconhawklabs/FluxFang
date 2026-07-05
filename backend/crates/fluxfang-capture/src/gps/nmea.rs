//! Pure parsing of NMEA 0183 sentences into a [`GpsFix`].
//!
//! [`parse_nmea`] is deliberately hardware/clock-free: it takes a single
//! sentence line plus a caller-supplied `date` and returns
//! `Option<GpsFix>`, `None` on anything it doesn't understand or that looks
//! truncated/invalid. This lets it be unit-tested with plain strings (see
//! `tests/nmea.rs`) with no serial device involved -
//! [`super::serial::SerialGpsSource`] is a thin wrapper that only adds
//! hardware I/O around this function.
//!
//! ## Sentences handled
//!
//! Only two talker/sentence combinations are recognized (YAGNI - this is all
//! FluxFang needs for a location + heading + speed track):
//!
//! - **`$GPGGA`** (Global Positioning System Fix Data): time, lat/lon,
//!   fix quality, satellite count, HDOP, altitude. Supplies `altitude` and
//!   `quality` on the returned fix; `speed`/`heading` are always `None`
//!   (GGA doesn't carry them).
//! - **`$GPRMC`** (Recommended Minimum Specific GNSS Data): time, status,
//!   lat/lon, speed over ground (knots), track angle (heading), date.
//!   Supplies `speed`/`heading`; `altitude` is always `None` (RMC doesn't
//!   carry it). `quality` is set to `1` for any valid (`status == 'A'`) RMC
//!   fix, since RMC has no fix-quality indicator of its own - see the note
//!   on [`parse_rmc`].
//!
//! Any other sentence (`$GPGSA`, `$GPGSV`, ...) returns `None`.
//!
//! ## No fix
//!
//! A GGA sentence with `quality == 0` (no fix) or an RMC sentence with
//! `status == 'V'` (void/invalid) returns `None` rather than a `GpsFix`
//! with placeholder coordinates.
//!
//! ## Units
//!
//! - Lat/lon are decimal degrees, negative for S/W (see [`parse_coord`]).
//! - `altitude` is meters (GGA's own unit; we don't convert - GGA's altitude
//!   units field is asserted to be `M` implicitly by the format but not
//!   checked, since every consumer FluxFang cares about wires up a `M`-unit
//!   GPS anyway - YAGNI).
//! - `speed` is meters/second. RMC reports speed over ground in **knots**;
//!   we convert with [`KNOTS_TO_MPS`] so every [`GpsFix::speed`] in this
//!   codebase (mock, gpsd, serial) is consistently m/s.
//! - `heading` is degrees, 0-360, true north.
//!
//! ## Date handling (why `parse_nmea` takes a `date` parameter)
//!
//! NMEA GGA sentences carry only a time-of-day (`hhmmss[.sss]`), no date.
//! RMC sentences *do* carry a `ddmmyy` date field, but for simplicity and to
//! keep both sentence types on one code path we deliberately ignore it and
//! always use the caller-supplied `date` for both - see [`parse_rmc`].
//! `parse_nmea` never calls `Utc::now()` or otherwise touches the clock, so
//! it stays pure and deterministic for unit testing. Callers that need a
//! "real" date (e.g. [`super::serial::SerialGpsSource`]) sample the clock
//! themselves, once per line, and pass the result in - that impurity lives
//! in the thin, untested-by-design I/O wrapper, not here.
//!
//! Sub-second precision in the time field (e.g. the `.00` in `123519.00`)
//! is dropped; `at` has one-second resolution. This is a deliberate
//! simplification (YAGNI) - nothing downstream needs sub-second GPS
//! timestamps.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use crate::GpsFix;

/// Knots -> meters/second conversion factor (1 knot = 1852 m / 3600 s).
const KNOTS_TO_MPS: f64 = 0.514444;

/// Parse a single NMEA 0183 sentence line into a [`GpsFix`].
///
/// `date` is combined with the sentence's time-of-day field to produce
/// `GpsFix::at` (see the module docs for why NMEA needs this injected rather
/// than reading the clock). Returns `None` for:
/// - sentences other than `$GPGGA`/`$GPRMC`,
/// - a GGA with `quality == 0` or an RMC with `status == 'V'` (no fix),
/// - anything malformed, short, or with unparseable numeric fields.
///
/// Never panics on malformed input.
pub fn parse_nmea(line: &str, date: NaiveDate) -> Option<GpsFix> {
    let fields: Vec<&str> = line.trim().split(',').collect();
    match *fields.first()? {
        "$GPGGA" => parse_gga(&fields, date),
        "$GPRMC" => parse_rmc(&fields, date),
        _ => None,
    }
}

/// `$GPGGA,time,lat,N/S,lon,E/W,quality,numSat,hdop,altitude,M,geoidSep,M,age,stationId*checksum`
fn parse_gga(fields: &[&str], date: NaiveDate) -> Option<GpsFix> {
    // Indices 0..=9 must exist (through altitude); trailing fields
    // (geoid separation, age, station id, checksum) are ignored.
    if fields.len() < 10 {
        return None;
    }
    let time = parse_time_of_day(fields[1])?;
    let lat = parse_coord(fields[2], fields[3], 2)?;
    let lon = parse_coord(fields[4], fields[5], 3)?;
    let quality: i32 = fields[6].trim().parse().ok()?;
    if quality == 0 {
        // Fix quality 0 = "invalid" per the NMEA spec - no usable position.
        return None;
    }
    let altitude = fields[9].trim().parse::<f64>().ok();

    Some(GpsFix {
        at: combine(date, time),
        lon,
        lat,
        altitude,
        speed: None,
        heading: None,
        quality,
    })
}

/// `$GPRMC,time,status,lat,N/S,lon,E/W,speedKnots,heading,date,magVar,E/W*checksum`
///
/// RMC's own `date` field (index 9, `ddmmyy`) is intentionally ignored in
/// favor of the caller-supplied `date` - see the module docs. RMC has no
/// fix-quality indicator, so a valid (`status == 'A'`) fix is stamped with
/// `quality = 1` (an arbitrary but documented "valid autonomous fix"
/// placeholder, matching the smallest non-zero GGA quality code).
fn parse_rmc(fields: &[&str], date: NaiveDate) -> Option<GpsFix> {
    // Indices 0..=8 must exist (through heading); date/magnetic variation
    // fields are ignored.
    if fields.len() < 9 {
        return None;
    }
    if fields[2].trim() != "A" {
        // 'V' (void) or anything else: no valid fix.
        return None;
    }
    let time = parse_time_of_day(fields[1])?;
    let lat = parse_coord(fields[3], fields[4], 2)?;
    let lon = parse_coord(fields[5], fields[6], 3)?;
    let speed = fields[7]
        .trim()
        .parse::<f64>()
        .ok()
        .map(|knots| knots * KNOTS_TO_MPS);
    let heading = fields[8].trim().parse::<f64>().ok();

    Some(GpsFix {
        at: combine(date, time),
        lon,
        lat,
        altitude: None,
        speed,
        heading,
        quality: 1,
    })
}

/// Combine a caller-supplied date with a sentence's time-of-day into a UTC
/// timestamp. NMEA times are already UTC, so this is a direct
/// reinterpretation, not a timezone conversion.
fn combine(date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    Utc.from_utc_datetime(&NaiveDateTime::new(date, time))
}

/// Parse an NMEA `hhmmss[.sss]` time-of-day field. Sub-second precision (the
/// optional `.sss` suffix) is dropped - see the module docs.
fn parse_time_of_day(raw: &str) -> Option<NaiveTime> {
    if raw.len() < 6 {
        return None;
    }
    let hh: u32 = raw.get(0..2)?.parse().ok()?;
    let mm: u32 = raw.get(2..4)?.parse().ok()?;
    let ss: u32 = raw.get(4..6)?.parse().ok()?;
    NaiveTime::from_hms_opt(hh, mm, ss)
}

/// Convert an NMEA `ddmm.mmmm` (lat, `degree_digits == 2`) or `dddmm.mmmm`
/// (lon, `degree_digits == 3`) coordinate plus its hemisphere letter
/// (`N`/`S`/`E`/`W`) into signed decimal degrees. `S` and `W` negate the
/// value; anything else (including an empty/unexpected hemisphere field)
/// is treated as positive (`N`/`E`).
fn parse_coord(raw: &str, hemisphere: &str, degree_digits: usize) -> Option<f64> {
    if raw.len() <= degree_digits {
        return None;
    }
    // `str::split_at` panics if `degree_digits` isn't on a UTF-8 char
    // boundary (possible with garbled/malicious input containing
    // multi-byte characters). Use the byte-safe, Option-returning
    // `get` instead - same pattern as `parse_time_of_day` above.
    let deg_str = raw.get(..degree_digits)?;
    let min_str = raw.get(degree_digits..)?;
    let degrees: f64 = deg_str.parse().ok()?;
    let minutes: f64 = min_str.parse().ok()?;
    let value = degrees + minutes / 60.0;
    Some(match hemisphere.trim() {
        "S" | "W" => -value,
        _ => value,
    })
}

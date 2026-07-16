//! GPS sources: pure NMEA sentence parsing ([`nmea`]) plus thin
//! network/hardware wrappers for `gpsd` ([`gpsd`]) and a serial NMEA device
//! ([`serial`]).
//!
//! See [`nmea::parse_nmea`] for the testable core. [`gpsd::GpsdSource`] and
//! [`serial::SerialGpsSource`] are not exercised by the automated test suite
//! (beyond serial's pure baud-rate validation) since they require a running
//! `gpsd` daemon / real serial hardware respectively - see their module docs.

pub mod gpsd;
pub mod manual;
pub mod nmea;
pub mod serial;

pub use gpsd::GpsdSource;
pub use manual::ManualGpsSource;
pub use nmea::parse_nmea;
pub use serial::{SerialGpsSource, ALLOWED_BAUD_RATES};

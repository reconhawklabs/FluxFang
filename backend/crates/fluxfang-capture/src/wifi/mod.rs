//! WiFi monitor-mode capture: pure frame parsing ([`parse`]) plus a thin
//! live-hardware wrapper ([`monitor`]).
//!
//! See [`parse::parse_frame`] for the testable core and
//! [`monitor::WifiMonitorCapturer`] for the `Capturer` impl that drives it
//! from a real interface.

pub mod monitor;
pub mod parse;

pub use monitor::WifiMonitorCapturer;
pub use parse::{parse_frame, WifiObservation};

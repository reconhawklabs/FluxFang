//! WiFi capture, in two modes:
//!
//! - **monitor** ([`monitor`]/[`parse`]): pure radiotap+802.11 frame
//!   parsing ([`parse::parse_frame`]) plus a thin live-hardware wrapper
//!   ([`monitor::WifiMonitorCapturer`]) that puts the interface into
//!   monitor mode and reads raw frames via libpcap.
//! - **scan** ([`scan`]): pure `iw dev <if> scan` output parsing
//!   ([`scan::parse_iw_scan`]) plus a thin live-hardware wrapper
//!   ([`scan::WifiScanCapturer`]) that periodically polls a plain
//!   managed-mode interface instead — see that module's docs for the
//!   trade-offs vs. monitor mode.

pub mod monitor;
pub mod parse;
pub mod scan;
pub mod security;

pub use monitor::WifiMonitorCapturer;
pub use parse::{parse_frame, WifiObservation};
pub use scan::{parse_iw_scan, WifiScanCapturer};

//! RTL-SDR capture. First mode: TPMS via the external `rtl_433` decoder.
//!
//! The pure JSON-line parser is [`parse::parse_tpms_line`]; the
//! subprocess-spawning half is [`tpms::TpmsCapturer`] (not unit-tested, same
//! convention as the wifi/bluetooth capturers).

pub mod parse;
pub mod tpms;

pub use tpms::{rtl_433_args, TpmsCapturer};

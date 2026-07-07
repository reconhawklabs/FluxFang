//! Bluetooth Scanning-mode capture (see the design doc). The pure
//! payload-mapping half is [`props::device_props_to_observation`]; the
//! D-Bus half is [`scan::BluetoothScanCapturer`].

pub mod props;
pub mod scan;

pub use scan::BluetoothScanCapturer;

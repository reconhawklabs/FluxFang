//! Bluetooth domain helpers shared across the app (pure, no I/O): the
//! compiled-in vendor / device-type lookup. Classification of bluetooth
//! payloads lives in `crate::classify` alongside the wifi classifiers.

pub mod vendor;

pub use vendor::{appearance_device_type, cod_device_type, company_name, oui_vendor};

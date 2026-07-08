//! `GET /api/system/capture-devices`: hardware enumeration for the WebUI's
//! data-source setup forms, so an operator picks a wireless interface /
//! serial GPS device from a dropdown instead of typing a name they have to
//! already know. PROTECTED — mounted in `lib.rs::app`'s protected router
//! group, behind `require_auth`, same as every other non-setup/login route.
//!
//! The handler is a thin wrapper around
//! `fluxfang_capture::enumerate::{list_wifi_interfaces, list_serial_devices,
//! list_bluetooth_adapters, list_rtl_sdr_devices}` — see that module's docs for the
//! `/sys/class/net` + `iw dev` fallback wifi-detection strategy, the
//! `/dev/serial/by-id` + `/dev/ttyUSB*`/`/dev/ttyACM*` serial-detection
//! strategy, the bluetooth adapter enumeration strategy, and the RTL-SDR device enumeration.
//! All enumeration functions are documented as never panicking and returning an
//! empty `Vec` when nothing is found (missing `/sys`, missing `iw` binary,
//! no serial devices, no bluetooth adapters, no RTL-SDR devices, ...), so this handler always
//! returns `200` — a hardware-less host (e.g. this very CI/dev container)
//! legitimately gets back
//! `{"wifi_interfaces":[],"serial_devices":[],"bluetooth_interfaces":[],"rtl_sdr_devices":[]}`
//! rather than an error.

use axum::routing::get;
use axum::{Json, Router};

use fluxfang_capture::enumerate::{
    list_bluetooth_adapters, list_rtl_sdr_devices, list_serial_devices, list_wifi_interfaces,
};

use crate::dto::CaptureDevicesDto;
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/system/capture-devices", get(capture_devices))
}

async fn capture_devices() -> Json<CaptureDevicesDto> {
    Json(CaptureDevicesDto {
        wifi_interfaces: list_wifi_interfaces(),
        serial_devices: list_serial_devices(),
        bluetooth_interfaces: list_bluetooth_adapters(),
        rtl_sdr_devices: list_rtl_sdr_devices(),
    })
}

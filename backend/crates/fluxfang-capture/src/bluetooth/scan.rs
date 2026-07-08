//! BlueZ (`org.bluez`, D-Bus) Scanning-mode capturer. Connects to the system
//! bus, selects the adapter, starts discovery (active when `active_scan`,
//! otherwise a passive advertisement scan), and maps each discovered
//! `org.bluez.Device1` snapshot to a `RawObservation` via
//! [`super::props::device_props_to_observation`].
//!
//! Not unit-tested (needs a live bus + adapter), same convention as the wifi
//! capturers. Runs continuously until `stop()`. A per-device throttle keeps
//! BlueZ's frequent `PropertiesChanged` (RSSI) updates from flooding
//! ingest: a device re-emits at most once per `EMIT_THROTTLE` unless a
//! meaningful field changed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc;

use zbus::export::futures_util::StreamExt;
use zbus::fdo::ObjectManagerProxy;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{proxy, Connection, MatchRule, MessageStream};

use super::props::{device_props_to_observation, DeviceProps};
use crate::{Capturer, RawObservation};

/// Minimum time between successive emissions for the same device address
/// when only RSSI is changing. BlueZ fires `PropertiesChanged` on every RSSI
/// tick (many/second); without this, one busy device would dominate ingest.
const EMIT_THROTTLE: Duration = Duration::from_secs(5);

/// The `org.bluez.Device1` D-Bus interface name — used both to pick the
/// device entry out of ObjectManager payloads and to arg0-match
/// `PropertiesChanged` signals.
const DEVICE_IFACE: &str = "org.bluez.Device1";

/// Whether BlueZ's `AdvertisementMonitorManager1.SupportedMonitorTypes`
/// includes a monitor type we can use for an all-devices passive scan. We use
/// `or_patterns`, the widely supported type. Absent/empty → no passive support.
fn passive_supported(supported_types: &[String]) -> bool {
    supported_types.iter().any(|t| t == "or_patterns")
}

/// A minimal proxy for `org.bluez.Adapter1` — just the bits the discovery
/// loop drives (power the adapter on, scope the scan, start/stop discovery).
#[proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
trait Adapter1 {
    fn start_discovery(&self) -> zbus::Result<()>;
    fn stop_discovery(&self) -> zbus::Result<()>;
    fn set_discovery_filter(&self, filter: HashMap<&str, Value<'_>>) -> zbus::Result<()>;

    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}

pub struct BluetoothScanCapturer {
    interface: String,
    active_scan: bool,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl BluetoothScanCapturer {
    pub fn new(interface: impl Into<String>, active_scan: bool) -> Self {
        Self {
            interface: interface.into(),
            active_scan,
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }
}

impl Capturer for BluetoothScanCapturer {
    fn start(&mut self, tx: mpsc::Sender<RawObservation>) -> anyhow::Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("capturer already running");
        }
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let interface = self.interface.clone();
        let active_scan = self.active_scan;

        // zbus is async; the Capturer seam is sync (like the wifi capturers,
        // which spawn a std::thread). Run a current-thread tokio runtime in
        // the thread and block on the D-Bus loop.
        let handle = thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("bluetooth scan: failed to build runtime: {err:#}");
                    return;
                }
            };
            if let Err(err) = rt.block_on(run_discovery(&interface, active_scan, &running, &tx)) {
                eprintln!("bluetooth scan on {interface}: {err:#}");
            }
        });
        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The D-Bus discovery loop. Talks to `org.bluez` on the system bus:
/// power on the adapter, `SetDiscoveryFilter` (Transport=auto, DuplicateData
/// per `active_scan`), `StartDiscovery`, then stream device property
/// snapshots (via ObjectManager `InterfacesAdded` + per-device
/// `PropertiesChanged`), building a `DeviceProps` for each and forwarding
/// `device_props_to_observation`'s result on `tx`. Honors `running` for
/// cooperative shutdown and `StopDiscovery` on exit.
///
/// Passive vs active caveat: BlueZ discovery is active by default. Enforcing
/// a *passive* scan type requires registering an
/// `org.bluez.AdvertisementMonitor1` all-match monitor, which needs a
/// server-side D-Bus object and is not wired here — so when `!active_scan`
/// this logs the limitation and proceeds with default discovery (see the
/// design doc's passive caveat; the data source records this in its status).
/// The `SetDiscoveryFilter` `DuplicateData` flag is still varied by
/// `active_scan` so a passive-intent scan at least suppresses duplicate
/// advertisement reports.
async fn run_discovery(
    interface: &str,
    active_scan: bool,
    running: &AtomicBool,
    tx: &mpsc::Sender<RawObservation>,
) -> anyhow::Result<()> {
    let conn = Connection::system().await?;

    // Select the adapter object, e.g. `/org/bluez/hci0`.
    let adapter_path = format!("/org/bluez/{interface}");
    let adapter = Adapter1Proxy::builder(&conn)
        .path(adapter_path.clone())?
        .build()
        .await?;

    // Best-effort power-on: an already-powered adapter (or a policy that
    // forbids toggling it) shouldn't abort the whole scan.
    if let Err(err) = adapter.set_powered(true).await {
        eprintln!("bluetooth scan on {interface}: could not power on adapter: {err:#}");
    }

    // Scope the scan. `Transport=auto` covers BR/EDR + LE; `DuplicateData`
    // controls whether BlueZ re-reports identical advertisements (wanted for
    // an active survey, suppressed for a lighter passive-intent scan).
    let mut filter: HashMap<&str, Value> = HashMap::new();
    filter.insert("Transport", Value::from("auto"));
    filter.insert("DuplicateData", Value::from(active_scan));
    if let Err(err) = adapter.set_discovery_filter(filter).await {
        eprintln!("bluetooth scan on {interface}: SetDiscoveryFilter failed: {err:#}");
    }

    if !active_scan {
        // See the function-level caveat: no AdvertisementMonitor1 wiring yet.
        eprintln!(
            "bluetooth scan on {interface}: passive advertisement monitoring is not \
             implemented; proceeding with default (active) discovery"
        );
    }

    adapter.start_discovery().await?;

    // ObjectManager on `/` surfaces every `org.bluez.Device1` object as it
    // appears (InterfacesAdded) and lets us seed already-known devices.
    let object_manager = ObjectManagerProxy::builder(&conn)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;

    // Per object-path snapshot cache (so a PropertiesChanged carrying only
    // RSSI can be merged onto the last full snapshot) + per-address throttle
    // state (last emit instant + last emitted props, to bypass the throttle
    // when a non-RSSI field actually changed).
    let mut cache: HashMap<String, DeviceProps> = HashMap::new();
    let mut last_emit: HashMap<String, (Instant, DeviceProps)> = HashMap::new();

    // Seed with devices BlueZ already knows about (cached from prior scans).
    if let Ok(objects) = object_manager.get_managed_objects().await {
        for (path, ifaces) in objects {
            for (iface_name, props) in ifaces {
                if iface_name.as_str() != DEVICE_IFACE {
                    continue;
                }
                let mut dp = DeviceProps::default();
                for (key, value) in props.iter() {
                    merge_prop(&mut dp, key, value);
                }
                cache.insert(path.to_string(), dp.clone());
                if !emit_if_due(&dp, &mut last_emit, tx).await {
                    return Ok(());
                }
            }
        }
    }

    let mut added = object_manager.receive_interfaces_added().await?;

    // Whole-bus `PropertiesChanged` for `org.bluez.Device1` (arg0-matched):
    // BlueZ reports RSSI/TxPower/etc. updates for known devices this way,
    // not through ObjectManager.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.bluez")?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .arg(0, DEVICE_IFACE)?
        .build();
    let mut changed = MessageStream::for_match_rule(rule, &conn, Some(256)).await?;

    // Wake up periodically even when the bus is quiet so cooperative
    // shutdown (`running` going false) is noticed promptly.
    let mut tick = tokio::time::interval(Duration::from_millis(500));

    while running.load(Ordering::SeqCst) {
        tokio::select! {
            _ = tick.tick() => {}

            item = added.next() => {
                let Some(signal) = item else { break; };
                let Ok(args) = signal.args() else { continue; };
                let Some(dev_props) = args.interfaces_and_properties().get(DEVICE_IFACE) else {
                    continue;
                };
                let path = args.object_path().to_string();
                let entry = cache.entry(path).or_default();
                for (key, value) in dev_props.iter() {
                    merge_prop(entry, key, value);
                }
                let snapshot = entry.clone();
                if !emit_if_due(&snapshot, &mut last_emit, tx).await {
                    break;
                }
            }

            item = changed.next() => {
                let Some(Ok(msg)) = item else { continue; };
                let path = match msg.header().path() {
                    Some(p) => p.to_string(),
                    None => continue,
                };
                let Ok((iface, props, _invalidated)) = msg
                    .body()
                    .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                else {
                    continue;
                };
                if iface != DEVICE_IFACE {
                    continue;
                }
                let entry = cache.entry(path).or_default();
                for (key, value) in props.iter() {
                    merge_prop(entry, key, value);
                }
                let snapshot = entry.clone();
                if !emit_if_due(&snapshot, &mut last_emit, tx).await {
                    break;
                }
            }
        }
    }

    let _ = adapter.stop_discovery().await;
    Ok(())
}

/// Emit `props` on `tx` unless throttled. Returns `false` only if the
/// receiver is gone (caller should stop). A device re-emits at most once per
/// [`EMIT_THROTTLE`] unless a non-RSSI field changed since its last emit.
async fn emit_if_due(
    props: &DeviceProps,
    last_emit: &mut HashMap<String, (Instant, DeviceProps)>,
    tx: &mpsc::Sender<RawObservation>,
) -> bool {
    let Some(obs) = device_props_to_observation(props, Utc::now()) else {
        // No address → no stable identity; nothing to emit, keep going.
        return true;
    };
    let Some(addr) = props.address.as_ref().map(|a| a.to_ascii_lowercase()) else {
        return true;
    };

    let now = Instant::now();
    if let Some((prev_at, prev_props)) = last_emit.get(&addr) {
        if now.duration_since(*prev_at) < EMIT_THROTTLE && !non_rssi_changed(prev_props, props) {
            return true; // throttled RSSI-only churn — skip.
        }
    }
    last_emit.insert(addr, (now, props.clone()));
    tx.send(obs).await.is_ok()
}

/// Whether anything other than RSSI differs between two snapshots — used to
/// bypass the RSSI-churn throttle when a meaningful field (name, UUIDs,
/// manufacturer data, tx power, …) actually changed.
fn non_rssi_changed(old: &DeviceProps, new: &DeviceProps) -> bool {
    let mut a = old.clone();
    let mut b = new.clone();
    a.rssi = None;
    b.rssi = None;
    a != b
}

/// Merge one `org.bluez.Device1` property (`key` = property name, `value` =
/// its D-Bus value) into `dp`. Unknown keys and type mismatches are ignored,
/// so a malformed/partial snapshot degrades gracefully rather than failing.
fn merge_prop(dp: &mut DeviceProps, key: &str, value: &Value) {
    match key {
        "Address" => {
            if let Value::Str(s) = value {
                dp.address = Some(s.as_str().to_string());
            }
        }
        "AddressType" => {
            if let Value::Str(s) = value {
                dp.address_type = Some(s.as_str().to_string());
            }
        }
        "Name" => {
            if let Value::Str(s) = value {
                dp.name = Some(s.as_str().to_string());
            }
        }
        "RSSI" => {
            if let Value::I16(n) = value {
                dp.rssi = Some(*n as i32);
            }
        }
        "TxPower" => {
            if let Value::I16(n) = value {
                dp.tx_power = Some(*n as i32);
            }
        }
        "Appearance" => {
            if let Value::U16(n) = value {
                dp.appearance = Some(*n);
            }
        }
        "Class" => {
            if let Value::U32(n) = value {
                dp.class_of_device = Some(*n);
            }
        }
        "UUIDs" => {
            if let Value::Array(arr) = value {
                dp.uuids = arr
                    .inner()
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.as_str().to_string()),
                        _ => None,
                    })
                    .collect();
            }
        }
        "ManufacturerData" => {
            if let Value::Dict(dict) = value {
                dp.manufacturer_data.clear();
                for (k, v) in dict.iter() {
                    if let Value::U16(company_id) = k {
                        dp.manufacturer_data.insert(*company_id, value_to_bytes(v));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Extract a byte vector from a BlueZ `ManufacturerData` value, unwrapping the
/// `a{qv}` variant layer if present. Non-byte entries are dropped.
fn value_to_bytes(value: &Value) -> Vec<u8> {
    let inner = match value {
        Value::Value(boxed) => boxed.as_ref(),
        other => other,
    };
    match inner {
        Value::Array(arr) => arr
            .inner()
            .iter()
            .filter_map(|v| match v {
                Value::U8(b) => Some(*b),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_supported_true_when_or_patterns_present() {
        assert!(passive_supported(&["or_patterns".to_string()]));
        assert!(passive_supported(&[
            "unknown".to_string(),
            "or_patterns".to_string()
        ]));
    }

    #[test]
    fn passive_supported_false_when_absent_or_empty() {
        assert!(!passive_supported(&[]));
        assert!(!passive_supported(&["rssi".to_string()]));
    }
}

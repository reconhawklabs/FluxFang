//! BlueZ (`org.bluez`, D-Bus) Scanning-mode capturer. Connects to the system
//! bus, selects the adapter, and either starts active discovery (when
//! `active_scan`) or registers an `org.bluez.AdvertisementMonitor1` for a
//! genuinely passive, listen-only scan (no `SCAN_REQ`) otherwise. Each
//! discovered `org.bluez.Device1` snapshot maps to a `RawObservation` via
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

/// How long `Capturer::start` waits for the D-Bus loop to report that the scan
/// actually started (active discovery began, or a passive monitor activated)
/// before treating it as a failed start. Must exceed `ACTIVATE_TIMEOUT` so a
/// passive-activation timeout surfaces as its specific reason, not this one.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the passive branch waits for BlueZ to call `Activate()` on our
/// registered AdvertisementMonitor before declaring passive scanning
/// unsupported on this adapter.
const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(8);

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

/// Proxy for `org.bluez.AdvertisementMonitorManager1` (lives on the adapter
/// object). Passive scanning is driven by registering a monitor here; BlueZ
/// then runs a background passive LE scan (no `SCAN_REQ`).
#[proxy(
    interface = "org.bluez.AdvertisementMonitorManager1",
    default_service = "org.bluez"
)]
trait AdvMonitorManager {
    fn register_monitor(&self, application: &zbus::zvariant::ObjectPath<'_>)
        -> zbus::Result<()>;
    fn unregister_monitor(&self, application: &zbus::zvariant::ObjectPath<'_>)
        -> zbus::Result<()>;

    #[zbus(property)]
    fn supported_monitor_types(&self) -> zbus::Result<Vec<String>>;
}

/// The object path root we register with BlueZ; it hosts an ObjectManager and
/// the single monitor object beneath it.
const MONITOR_ROOT: &str = "/org/fluxfang/advmon";
/// The monitor object path (child of `MONITOR_ROOT`).
const MONITOR_PATH: &str = "/org/fluxfang/advmon/monitor0";

/// Broadest `or_patterns` set BlueZ will accept, used to catch as many
/// advertisers as possible. `or_patterns` requires at least one pattern (there
/// is no true wildcard), so we OR several near-ubiquitous AD types with empty
/// content (empty content matches presence of that AD structure). Coverage is
/// still controller-dependent — see the module caveat.
///
/// Tuple layout is BlueZ's `Patterns` element: (start_position, ad_data_type, content).
fn catch_all_patterns() -> Vec<(u8, u8, Vec<u8>)> {
    // 0x01 Flags, 0x02/0x03 incomplete/complete 16-bit UUIDs,
    // 0x08/0x09 shortened/complete local name, 0x16 service data,
    // 0xFF manufacturer-specific data.
    [0x01u8, 0x02, 0x03, 0x08, 0x09, 0x16, 0xFF]
        .into_iter()
        .map(|ad_type| (0u8, ad_type, Vec::new()))
        .collect()
}

/// Our `org.bluez.AdvertisementMonitor1` implementation. BlueZ calls `Activate`
/// once the monitor is accepted (that is our signal passive scanning is live),
/// `Release` when it drops the monitor, and `DeviceFound`/`DeviceLost` as the
/// passive scan sees devices. We harvest device data through the existing
/// ObjectManager/`PropertiesChanged` pipeline rather than from these callbacks,
/// so the callbacks only need to exist.
struct AdvMonitor {
    /// Fired from `Activate` to release the setup code waiting on activation.
    activated: Arc<tokio::sync::Notify>,
}

#[zbus::interface(name = "org.bluez.AdvertisementMonitor1")]
impl AdvMonitor {
    fn release(&self) {}

    fn activate(&self) {
        self.activated.notify_one();
    }

    fn device_found(&self, _device: zbus::zvariant::OwnedObjectPath) {}

    fn device_lost(&self, _device: zbus::zvariant::OwnedObjectPath) {}

    #[zbus(property)]
    fn type_(&self) -> &str {
        "or_patterns"
    }

    #[zbus(property)]
    fn patterns(&self) -> Vec<(u8, u8, Vec<u8>)> {
        catch_all_patterns()
    }
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

        // The D-Bus loop reports its startup outcome (scan actually began, or a
        // reason it could not) back over this channel, so a failure to start —
        // in particular an adapter that cannot scan passively — surfaces as an
        // Err from start() and becomes the data source's `last_error`.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

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
                    let _ = ready_tx.send(Err(format!(
                        "bluetooth scan on {interface}: failed to build runtime: {err:#}"
                    )));
                    return;
                }
            };
            if let Err(err) = rt.block_on(run_discovery(
                &interface,
                active_scan,
                &running,
                &tx,
                &ready_tx,
            )) {
                // Startup outcome was already reported via `ready_tx`; anything
                // here is a post-startup runtime error.
                eprintln!("bluetooth scan on {interface}: {err:#}");
            }
        });

        match interpret_startup(ready_rx.recv_timeout(STARTUP_TIMEOUT), &self.interface) {
            Ok(()) => {
                self.handle = Some(handle);
                Ok(())
            }
            Err(err) => {
                // Stop the loop and reap the thread so a failed start leaves no
                // orphaned scan running.
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Err(err)
            }
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Map the D-Bus loop thread's startup report into `Capturer::start`'s result.
/// `Ok(Ok(()))` → started; `Ok(Err(reason))` → the loop reported a startup
/// failure (surfaced verbatim to `last_error`); `Err(timeout)` → the loop never
/// reported in time.
fn interpret_startup(
    outcome: Result<Result<(), String>, std::sync::mpsc::RecvTimeoutError>,
    interface: &str,
) -> anyhow::Result<()> {
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(reason)) => Err(anyhow::anyhow!(reason)),
        Err(_) => Err(anyhow::anyhow!(
            "bluetooth scan on {interface}: timed out waiting for scan startup"
        )),
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
/// Passive vs active: when `active_scan`, this runs BlueZ active discovery
/// (issues `SCAN_REQ`). Otherwise it registers an all-devices
/// `AdvertisementMonitor1` and lets BlueZ run a listen-only passive scan — it
/// never calls `StartDiscovery`, so the adapter transmits nothing. If the
/// adapter cannot scan passively (no AdvertisementMonitor support, registration
/// fails, or activation times out) this reports the reason over `ready` and
/// returns without scanning, so the data source fails loudly rather than
/// silently transmitting. Passive-scan completeness is controller-dependent
/// (offloaded filtering may drop non-matching advertisers); the complete
/// passive path is the future nRF Sniffing mode.
async fn run_discovery(
    interface: &str,
    active_scan: bool,
    running: &AtomicBool,
    tx: &mpsc::Sender<RawObservation>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
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

    if active_scan {
        // Active survey: richest data. `Transport=auto` covers BR/EDR + LE;
        // `DuplicateData=true` re-reports identical advertisements.
        let mut filter: HashMap<&str, Value> = HashMap::new();
        filter.insert("Transport", Value::from("auto"));
        filter.insert("DuplicateData", Value::from(true));
        if let Err(err) = adapter.set_discovery_filter(filter).await {
            eprintln!("bluetooth scan on {interface}: SetDiscoveryFilter failed: {err:#}");
        }
        adapter.start_discovery().await?;
        // Active discovery is running — report a successful start.
        let _ = ready.send(Ok(()));
    } else {
        // Passive: register an AdvertisementMonitor and let BlueZ run a
        // listen-only scan. Never call StartDiscovery (that would transmit).
        // Any failure is reported as a precise reason and aborts the scan —
        // we do not fall back to active discovery.
        if let Err(reason) = setup_passive_monitor(interface, &conn, &adapter_path).await {
            let _ = ready.send(Err(reason));
            return Ok(());
        }
        let _ = ready.send(Ok(()));
    }

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

    if active_scan {
        let _ = adapter.stop_discovery().await;
    } else {
        // Best-effort unregister of the monitor, then drop the served objects.
        if let Ok(builder) = AdvMonitorManagerProxy::builder(&conn).path(adapter_path.clone()) {
            if let Ok(manager) = builder.build().await {
                if let Ok(root) = zbus::zvariant::ObjectPath::try_from(MONITOR_ROOT) {
                    let _ = manager.unregister_monitor(&root).await;
                }
            }
        }
        teardown_monitor(&conn).await;
    }
    Ok(())
}

/// Register an all-devices passive AdvertisementMonitor on `adapter_path` and
/// wait for BlueZ to `Activate` it. Returns `Err(reason)` (a user-facing string)
/// if the adapter has no AdvertisementMonitor support, registration fails, or
/// activation does not arrive within `ACTIVATE_TIMEOUT`. On failure it best-effort
/// tears down anything it created. Never starts active discovery.
async fn setup_passive_monitor(
    interface: &str,
    conn: &Connection,
    adapter_path: &str,
) -> Result<(), String> {
    let manager = AdvMonitorManagerProxy::builder(conn)
        .path(adapter_path.to_string())
        .map_err(|e| format!("bluetooth scan on {interface}: bad adapter path: {e:#}"))?
        .build()
        .await
        .map_err(|e| {
            format!(
                "bluetooth scan on {interface}: passive scan unsupported \
                 (no AdvertisementMonitorManager1): {e:#}. Enable Active Scanning \
                 to use this adapter."
            )
        })?;

    let supported = manager.supported_monitor_types().await.map_err(|e| {
        format!("bluetooth scan on {interface}: could not query SupportedMonitorTypes: {e:#}")
    })?;
    if !passive_supported(&supported) {
        return Err(format!(
            "bluetooth scan on {interface}: passive scan unsupported on this adapter \
             (SupportedMonitorTypes = {supported:?}). Enable Active Scanning to use it."
        ));
    }

    // Serve an ObjectManager root + the monitor object beneath it. BlueZ
    // enumerates monitors under the registered root via ObjectManager.
    let activated = Arc::new(tokio::sync::Notify::new());
    let server = conn.object_server();
    server
        .at(MONITOR_ROOT, zbus::fdo::ObjectManager)
        .await
        .map_err(|e| format!("bluetooth scan on {interface}: serving ObjectManager failed: {e:#}"))?;
    server
        .at(
            MONITOR_PATH,
            AdvMonitor {
                activated: activated.clone(),
            },
        )
        .await
        .map_err(|e| format!("bluetooth scan on {interface}: serving monitor failed: {e:#}"))?;

    let root = zbus::zvariant::ObjectPath::try_from(MONITOR_ROOT)
        .map_err(|e| format!("bluetooth scan on {interface}: bad monitor root path: {e:#}"))?;

    if let Err(e) = manager.register_monitor(&root).await {
        teardown_monitor(conn).await;
        return Err(format!(
            "bluetooth scan on {interface}: RegisterMonitor failed: {e:#}. \
             Enable Active Scanning to use this adapter."
        ));
    }

    // Wait for BlueZ to Activate the monitor — that is the confirmation the
    // passive scan is actually running.
    match tokio::time::timeout(ACTIVATE_TIMEOUT, activated.notified()).await {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = manager.unregister_monitor(&root).await;
            teardown_monitor(conn).await;
            Err(format!(
                "bluetooth scan on {interface}: passive monitor was not activated within \
                 {}s; adapter likely does not support passive scanning. Enable Active Scanning.",
                ACTIVATE_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Best-effort removal of the objects `setup_passive_monitor` exported.
async fn teardown_monitor(conn: &Connection) {
    let server = conn.object_server();
    let _ = server.remove::<AdvMonitor, _>(MONITOR_PATH).await;
    let _ = server.remove::<zbus::fdo::ObjectManager, _>(MONITOR_ROOT).await;
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
    use std::sync::mpsc::RecvTimeoutError;

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

    #[test]
    fn interpret_startup_ok_passes() {
        assert!(interpret_startup(Ok(Ok(())), "hci0").is_ok());
    }

    #[test]
    fn interpret_startup_err_propagates_reason() {
        let err =
            interpret_startup(Ok(Err("passive unsupported".to_string())), "hci0").unwrap_err();
        assert!(err.to_string().contains("passive unsupported"));
    }

    #[test]
    fn interpret_startup_timeout_mentions_interface_and_timeout() {
        let err = interpret_startup(Err(RecvTimeoutError::Timeout), "hci9").unwrap_err();
        assert!(err.to_string().contains("hci9"));
        assert!(err.to_string().contains("timed out"));
    }
}

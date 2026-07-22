// Task 9.3: list configured capture devices (wifi monitor-mode interfaces,
// gps receivers), add new ones, and start/stop/delete them.
//
// Live status: the WS stream (`useLiveEvents`, Task 9.1) does not push
// data-source status changes in this slice — only `emission`/`notification`
// frames exist — so `stopped -> starting -> running`/`error` transitions
// (driven server-side by `CaptureSupervisor`, see backend
// `fluxfang-api::data_sources` module docs) would never be reflected here
// without polling. `REFETCH_INTERVAL_MS` below re-runs `listDataSources`
// on a short timer instead; if a later task adds a data-source WS frame,
// this poll can shrink or go away in favor of `queryClient.invalidateQueries`
// on that frame, same as `queryKeys.emissions`/`queryKeys.dashboard`.
import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { useConfig } from '../hooks/useConfig';
import {
  BAUD_RATES,
  RTL_FREQUENCIES,
  createDataSource,
  deleteDataSource,
  listCaptureDevices,
  listDataSources,
  startDataSource,
  stopDataSource,
  updateDataSource,
} from '../api/dataSources';
import type {
  BaudRate,
  CreateDataSourceInput,
  DataSource,
  DataSourceStatus,
  RtlFrequency,
} from '../api/dataSources';

/** How often to re-poll the list while this page is mounted (see module doc
 * comment on why polling, not WS, drives status here). A few seconds is
 * enough to make starting -> running/error feel responsive without hammering
 * the API. */
const REFETCH_INTERVAL_MS = 4000;

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';

const STATUS_BADGE_CLASSES: Record<DataSourceStatus, string> = {
  stopped: 'bg-slate-700 text-slate-300',
  starting: 'animate-pulse bg-amber-500/20 text-amber-400',
  running: 'bg-green-500/20 text-green-400',
  error: 'bg-red-500/20 text-red-400',
};

function StatusBadge({ status }: { status: DataSourceStatus }) {
  return (
    <span
      data-testid="status-badge"
      className={`inline-block rounded px-2 py-0.5 text-xs font-medium capitalize ${STATUS_BADGE_CLASSES[status]}`}
    >
      {status}
    </span>
  );
}

/** Honest health summary: `running` reads as healthy; an `error` while the
 * user still wants the source running (`desired_state === 'running'`) reads
 * as "retrying" (the backend's reconciler retries every 10s) plus how long
 * it's been down since `last_ok_at`, rather than a bare "error" that could
 * be mistaken for a permanent/manual state. Everything else (stopped,
 * starting, or an error the user has since stopped wanting) falls back to
 * the plain status. */
function HealthCell({ source }: { source: DataSource }) {
  if (source.status === 'running') {
    return <span className="text-xs text-green-400">● healthy</span>;
  }
  if (source.status === 'error' && source.desired_state === 'running') {
    const since = source.last_ok_at ? `, down ${relativeSince(source.last_ok_at)}` : '';
    return <span className="text-xs text-red-400">⚠ error — retrying{since}</span>;
  }
  return <span className="text-xs text-slate-500">○ {source.status}</span>;
}

/** Coarse relative-time string ("Ns"/"Nm"/"Nh") since an ISO timestamp — the
 * Health cell's "down for" duration. The page already re-polls every
 * `REFETCH_INTERVAL_MS`, so this recomputes on each render without extra
 * timers/wiring. */
function relativeSince(iso: string): string {
  const secs = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h`;
}

/** A short human summary of a source's interface/config, shown monospace
 * since it's device-ish identifying text (interface name / serial device /
 * host:port), not prose. */
function ConfigSummary({ source }: { source: DataSource }) {
  if (source.kind === 'wifi') {
    return <span className="font-mono text-slate-300">{source.interface}</span>;
  }
  if (source.mode === 'serial' && 'device' in source.config) {
    return (
      <>
        <span className="font-mono text-slate-300">{source.config.device}</span>
        <span className="text-slate-500"> @ {source.config.baud}</span>
      </>
    );
  }
  if (source.mode === 'gpsd' && 'host' in source.config) {
    return (
      <span className="font-mono text-slate-300">
        {source.config.host}:{source.config.port}
      </span>
    );
  }
  if (source.mode === 'manual' && 'lat' in source.config && 'lon' in source.config) {
    return (
      <span className="font-mono text-slate-300">
        {source.config.lat}, {source.config.lon}
      </span>
    );
  }
  if (source.kind === 'bluetooth') {
    const activeScan = 'active_scan' in source.config && source.config.active_scan === true;
    const autoCreate = 'auto_create_emitters' in source.config && source.config.auto_create_emitters === true;
    return (
      <>
        <span className="font-mono text-slate-300">{source.interface}</span>
        <span className="text-slate-500">
          {' '}
          ({activeScan ? 'active' : 'passive'}
          {autoCreate ? ', auto' : ''})
        </span>
      </>
    );
  }
  if (source.kind === 'rtl_sdr') {
    const freq = 'frequency' in source.config ? source.config.frequency : '?';
    const serial =
      'device_serial' in source.config && source.config.device_serial
        ? source.config.device_serial
        : 'dev 0';
    const autoCreate =
      'auto_create_emitters' in source.config && source.config.auto_create_emitters === true;
    return (
      <>
        <span className="font-mono text-slate-300">
          {freq} @ {serial}
        </span>
        <span className="text-slate-500">{autoCreate ? ' (auto)' : ''}</span>
      </>
    );
  }
  return <span className="text-slate-500">—</span>;
}

type FormKind = 'wifi' | 'gps' | 'bluetooth' | 'rtl_sdr' | 'sensor';
type FormWifiMode = 'monitor' | 'scan';
type FormGpsMode = 'gpsd' | 'serial' | 'manual';

/** One-line description shown under the WiFi Mode dropdown for whichever
 * mode is currently selected — see backend `fluxfang-api::capture`'s
 * `"monitor"`/`"scan"` capturer split (`WifiMonitorCapturer` vs
 * `WifiScanCapturer`). */
const WIFI_MODE_HELP: Record<FormWifiMode, string> = {
  monitor: 'Monitor Mode puts the adapter into monitor mode to capture all 802.11 frames.',
  scan: 'SSID Scan uses the adapter as-is to scan for nearby networks.',
};

const NO_WIFI_MESSAGE = 'No compatible WiFi card found.';
const NO_SERIAL_MESSAGE = 'No compatible serial GPS device found.';
const NO_BLUETOOTH_MESSAGE = 'No Bluetooth adapter found.';
const NO_RTL_MESSAGE = 'No RTL-SDR device found.';

/** Bluetooth ships with a single mode for this spec — the dropdown still
 * shows it (for a consistent Mode-select experience across kinds) but it's
 * effectively static since there's only one `<option>`. */
const BT_MODE_HELP = 'Passive BLE advertisement scanning via a host HCI adapter.';

/** RTL-SDR ships with a single mode (TPMS, via rtl_433) for this spec — same
 * static-dropdown rationale as `BT_MODE_HELP`. */
const RTL_MODE_HELP = 'TPMS decodes tire-pressure sensor reports via rtl_433.';

/** Sensor ships with a single mode (listener) — same static-dropdown
 * rationale as `BT_MODE_HELP`/`RTL_MODE_HELP`. */
const SENSOR_MODE_HELP =
  'A network listener that accepts connections from distributed Sensor nodes. Start it, then approve sensors on the Sensors page.';

interface AddSourceFormProps {
  onCancel: () => void;
  onSubmit: (input: CreateDataSourceInput) => void;
  submitting: boolean;
  errorMessage: string | null;
}

function AddSourceForm({ onCancel, onSubmit, submitting, errorMessage }: AddSourceFormProps) {
  // A Sensor node only captures and forwards raw emissions — it never hosts a
  // sensor listener (that's the Standalone's ingest point) and never builds
  // emitters (grouping happens on the Standalone at approval). So on a Sensor
  // node we hide the "Sensor" datasource kind and the auto-create-emitters
  // option entirely.
  const { data: config } = useConfig();
  const isSensor = config?.role === 'sensor';
  const [kind, setKind] = useState<FormKind>('wifi');
  const [wifiMode, setWifiMode] = useState<FormWifiMode>('monitor');
  const [iface, setIface] = useState('');
  // Phase B (emitter auto-classification design doc's Frontend section):
  // opt-in per source, defaults OFF. Applies to both wifi modes (monitor
  // captures both beacons and probe requests; scan only surfaces APs) —
  // either way the backend's ingest auto-create only fires when this is set.
  const [autoCreateEmitters, setAutoCreateEmitters] = useState(false);
  // Bluetooth-only: opt-in active (scan-request) BLE scanning vs. the
  // default passive advertisement listening. Defaults OFF, same rationale
  // as `autoCreateEmitters` above.
  const [btActiveScan, setBtActiveScan] = useState(false);
  const [gpsMode, setGpsMode] = useState<FormGpsMode>('gpsd');
  const [host, setHost] = useState('127.0.0.1');
  const [port, setPort] = useState('2947');
  const [device, setDevice] = useState('');
  const [baud, setBaud] = useState<BaudRate>(9600);
  const [manualLat, setManualLat] = useState('');
  const [manualLon, setManualLon] = useState('');
  // RTL-SDR / TPMS state.
  const [rtlFrequency, setRtlFrequency] = useState<RtlFrequency>('315M');
  const [rtlSerial, setRtlSerial] = useState('');
  const [tpmsAutoCorrelate, setTpmsAutoCorrelate] = useState(false);
  // Sensor listener state — a pure network bind, no hardware interface.
  const [bindIp, setBindIp] = useState('0.0.0.0');
  const [bindPort, setBindPort] = useState('9000');

  // Hardware enumeration (Task devdropdown) — the Add form no longer takes
  // free-text interface/device names; it offers only what
  // `GET /api/system/capture-devices` actually reports as present, so a
  // typo'd/nonexistent device can't be submitted in the first place.
  const devicesQuery = useQuery({
    queryKey: queryKeys.captureDevices,
    queryFn: listCaptureDevices,
  });

  const devicesLoading = devicesQuery.isLoading;
  const devicesErrored = devicesQuery.isError;
  const wifiInterfaces = devicesQuery.data?.wifi_interfaces ?? [];
  const serialDevices = devicesQuery.data?.serial_devices ?? [];
  const bluetoothInterfaces = devicesQuery.data?.bluetooth_interfaces ?? [];
  const wifiHasDevices = wifiInterfaces.length > 0;
  const serialHasDevices = serialDevices.length > 0;
  const bluetoothHasDevices = bluetoothInterfaces.length > 0;
  const rtlDevices = devicesQuery.data?.rtl_sdr_devices ?? [];
  const rtlHasDevices = rtlDevices.length > 0;

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();

    if (kind === 'wifi') {
      onSubmit({
        kind: 'wifi',
        mode: wifiMode,
        interface: iface,
        config: { auto_create_emitters: autoCreateEmitters },
      });
      return;
    }

    if (kind === 'bluetooth') {
      onSubmit({
        kind: 'bluetooth',
        mode: 'scan',
        interface: iface,
        config: { auto_create_emitters: autoCreateEmitters, active_scan: btActiveScan },
      });
      return;
    }

    if (kind === 'rtl_sdr') {
      onSubmit({
        kind: 'rtl_sdr',
        mode: 'tpms',
        config: {
          frequency: rtlFrequency,
          ...(rtlSerial ? { device_serial: rtlSerial } : {}),
          auto_create_emitters: autoCreateEmitters,
          auto_correlate_tpms: tpmsAutoCorrelate,
        },
      });
      return;
    }

    if (kind === 'sensor') {
      onSubmit({
        kind: 'sensor',
        mode: 'listener',
        config: {
          bind_ip: bindIp.trim(),
          bind_port: Number(bindPort),
        },
      });
      return;
    }

    if (gpsMode === 'gpsd') {
      onSubmit({ kind: 'gps', mode: 'gpsd', config: { host, port: Number(port) } });
      return;
    }

    if (gpsMode === 'manual') {
      onSubmit({
        kind: 'gps',
        mode: 'manual',
        config: { lat: Number(manualLat), lon: Number(manualLon) },
      });
      return;
    }

    onSubmit({ kind: 'gps', mode: 'serial', config: { device, baud } });
  }

  // The Add button is disabled whenever the currently-selected path has no
  // selectable device (wifi with an empty enumeration, or gps-serial with
  // an empty one) or the hardware list hasn't resolved yet (still loading,
  // or errored — never fall back to letting the user type a device name).
  let canSubmit: boolean;
  if (kind === 'wifi') {
    // Require the selection to still be one of the currently-enumerated
    // interfaces — guards against a stale pick after a Refresh changes the list.
    canSubmit = !devicesLoading && !devicesErrored && wifiInterfaces.includes(iface);
  } else if (kind === 'bluetooth') {
    canSubmit = !devicesLoading && !devicesErrored && bluetoothInterfaces.includes(iface);
  } else if (kind === 'rtl_sdr') {
    canSubmit =
      !devicesLoading &&
      !devicesErrored &&
      rtlDevices.some((d) => d.serial === rtlSerial);
  } else if (kind === 'sensor') {
    // No hardware enumeration for a pure network listener — validate the
    // form fields directly instead of gating on `devicesQuery`.
    const portNum = Number(bindPort);
    canSubmit =
      bindIp.trim() !== '' &&
      Number.isFinite(portNum) &&
      portNum >= 1 &&
      portNum <= 65535;
  } else if (gpsMode === 'serial') {
    canSubmit = !devicesLoading && !devicesErrored && serialDevices.includes(device);
  } else if (gpsMode === 'manual') {
    const lat = Number(manualLat);
    const lon = Number(manualLon);
    canSubmit =
      manualLat.trim() !== '' &&
      manualLon.trim() !== '' &&
      Number.isFinite(lat) &&
      Number.isFinite(lon) &&
      lat >= -90 &&
      lat <= 90 &&
      lon >= -180 &&
      lon <= 180;
  } else {
    canSubmit = host.trim() !== '' && port.trim() !== '';
  }

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-md space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold text-slate-100">Add Data Source</h2>
          <button
            type="button"
            onClick={() => void devicesQuery.refetch()}
            disabled={devicesQuery.isFetching}
            className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {devicesQuery.isFetching ? 'Refreshing…' : 'Refresh'}
          </button>
        </div>

        {devicesErrored && (
          <p role="alert" className="text-sm text-red-400">
            Failed to load hardware devices.{' '}
            <button
              type="button"
              onClick={() => void devicesQuery.refetch()}
              className="underline hover:text-red-300"
            >
              Retry
            </button>
          </p>
        )}

        <div className="space-y-1">
          <label htmlFor="ds-kind" className={labelClassName}>
            Kind
          </label>
          <select
            id="ds-kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as FormKind)}
            className={inputClassName}
          >
            <option value="wifi">Wifi</option>
            <option value="gps">GPS</option>
            <option value="bluetooth">Bluetooth</option>
            <option value="rtl_sdr">RTL-SDR</option>
            {!isSensor && <option value="sensor">Sensor (listener)</option>}
          </select>
        </div>

        {kind === 'wifi' && (
          <>
            <div className="space-y-1">
              <label htmlFor="ds-wifi-mode" className={labelClassName}>
                Mode
              </label>
              <select
                id="ds-wifi-mode"
                value={wifiMode}
                onChange={(event) => setWifiMode(event.target.value as FormWifiMode)}
                className={inputClassName}
              >
                <option value="monitor">Monitor Mode</option>
                <option value="scan">SSID Scan</option>
              </select>
              <p className="text-xs text-slate-500">{WIFI_MODE_HELP[wifiMode]}</p>
            </div>

            <div className="space-y-1">
              <label htmlFor="ds-interface" className={labelClassName}>
                Interface
              </label>
              {devicesLoading && <p className="text-sm text-slate-500">Loading interfaces…</p>}
              {!devicesLoading && !devicesErrored && wifiHasDevices && (
                <select
                  id="ds-interface"
                  value={iface}
                  onChange={(event) => setIface(event.target.value)}
                  className={`font-mono ${inputClassName}`}
                >
                  <option value="">Select an interface…</option>
                  {wifiInterfaces.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              )}
              {!devicesLoading && !devicesErrored && !wifiHasDevices && (
                <p className="text-sm text-amber-400">{NO_WIFI_MESSAGE}</p>
              )}
            </div>

            {!isSensor && (
              <div className="space-y-1">
                <label className="flex items-center gap-2 text-sm text-slate-200">
                  <input
                    id="ds-auto-create-emitters"
                    type="checkbox"
                    checked={autoCreateEmitters}
                    onChange={(event) => setAutoCreateEmitters(event.target.checked)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                  Automatically create emitters (by AP BSSID / client MAC)
                </label>
                <p className="text-xs text-slate-500">
                  When enabled, each new access point or client device seen on this source is
                  auto-registered as an emitter with a visible, toggleable match rule.
                </p>
              </div>
            )}
          </>
        )}

        {kind === 'bluetooth' && (
          <>
            <div className="space-y-1">
              <label htmlFor="ds-bt-mode" className={labelClassName}>
                Mode
              </label>
              <select id="ds-bt-mode" value="scan" disabled className={inputClassName}>
                <option value="scan">Scanning</option>
              </select>
              <p className="text-xs text-slate-500">{BT_MODE_HELP}</p>
            </div>

            <div className="space-y-1">
              <label htmlFor="ds-bt-interface" className={labelClassName}>
                Adapter
              </label>
              {devicesLoading && <p className="text-sm text-slate-500">Loading adapters…</p>}
              {!devicesLoading && !devicesErrored && bluetoothHasDevices && (
                <select
                  id="ds-bt-interface"
                  value={iface}
                  onChange={(event) => setIface(event.target.value)}
                  className={`font-mono ${inputClassName}`}
                >
                  <option value="">Select an adapter…</option>
                  {bluetoothInterfaces.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              )}
              {!devicesLoading && !devicesErrored && !bluetoothHasDevices && (
                <p className="text-sm text-amber-400">{NO_BLUETOOTH_MESSAGE}</p>
              )}
            </div>

            {!isSensor && (
              <div className="space-y-1">
                <label className="flex items-center gap-2 text-sm text-slate-200">
                  <input
                    id="ds-bt-auto-create-emitters"
                    type="checkbox"
                    checked={autoCreateEmitters}
                    onChange={(event) => setAutoCreateEmitters(event.target.checked)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                  Automatically create emitters (by device address)
                </label>
                <p className="text-xs text-slate-500">
                  When enabled, each new Bluetooth device seen on this source is auto-registered as
                  an emitter with a visible, toggleable match rule.
                </p>
              </div>
            )}

            <div className="space-y-1">
              <label className="flex items-center gap-2 text-sm text-slate-200">
                <input
                  id="ds-bt-active-scan"
                  type="checkbox"
                  checked={btActiveScan}
                  onChange={(event) => setBtActiveScan(event.target.checked)}
                  className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                />
                Enable Active Scanning
              </label>
              <p className="text-xs text-slate-500">
                Enabling Active Scanning makes the adapter transmit scan-request
                frames to probe devices for more data. Leaving it disabled keeps
                the Bluetooth adapter RF-quiet (listen-only) — but note that some
                adapters may fail to start if they do not support passive scanning
                and Active Scanning is disabled.
              </p>
            </div>
          </>
        )}

        {kind === 'rtl_sdr' && (
          <>
            <div className="space-y-1">
              <label htmlFor="ds-rtl-mode" className={labelClassName}>
                Mode
              </label>
              <select id="ds-rtl-mode" value="tpms" disabled className={inputClassName}>
                <option value="tpms">TPMS</option>
              </select>
              <p className="text-xs text-slate-500">{RTL_MODE_HELP}</p>
            </div>

            <div className="space-y-1">
              <label htmlFor="ds-rtl-frequency" className={labelClassName}>
                Frequency
              </label>
              <select
                id="ds-rtl-frequency"
                value={rtlFrequency}
                onChange={(event) => setRtlFrequency(event.target.value as RtlFrequency)}
                className={inputClassName}
              >
                {RTL_FREQUENCIES.map((f) => (
                  <option key={f.value} value={f.value}>
                    {f.label}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1">
              <label htmlFor="ds-rtl-device" className={labelClassName}>
                Device
              </label>
              {devicesLoading && <p className="text-sm text-slate-500">Loading devices…</p>}
              {!devicesLoading && !devicesErrored && rtlHasDevices && (
                <select
                  id="ds-rtl-device"
                  value={rtlSerial}
                  onChange={(event) => setRtlSerial(event.target.value)}
                  className={`font-mono ${inputClassName}`}
                >
                  <option value="">Select a device…</option>
                  {rtlDevices.map((d) => (
                    <option key={d.serial} value={d.serial}>
                      index {d.index} — {d.name} (SN: {d.serial})
                    </option>
                  ))}
                </select>
              )}
              {!devicesLoading && !devicesErrored && !rtlHasDevices && (
                <p className="text-sm text-amber-400">{NO_RTL_MESSAGE}</p>
              )}
            </div>

            {!isSensor && (
              <div className="space-y-1">
                <label className="flex items-center gap-2 text-sm text-slate-200">
                  <input
                    id="ds-rtl-auto-create-emitters"
                    type="checkbox"
                    checked={autoCreateEmitters}
                    onChange={(event) => setAutoCreateEmitters(event.target.checked)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                  Automatically create emitters (one per TPMS sensor id)
                </label>
                <p className="text-xs text-slate-500">
                  When enabled, each new tire-sensor id seen on this source is auto-registered as a
                  TPMS Sensor emitter named TPMS_&lt;id&gt;.
                </p>
              </div>
            )}

            <div className="space-y-1">
              <label className="flex items-center gap-2 text-sm text-slate-200">
                <input
                  id="ds-tpms-auto-correlate"
                  type="checkbox"
                  checked={tpmsAutoCorrelate}
                  onChange={(event) => setTpmsAutoCorrelate(event.target.checked)}
                  className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                />
                Attempt to Connect TPMS Emitters to Other Tires
              </label>
              <p className="text-xs text-slate-500">
                When enabled, the correlation engine tries to group tire sensors seen travelling
                together into the same vehicle. (Manual associations are always available on each
                sensor's detail page.)
              </p>
            </div>
          </>
        )}

        {kind === 'sensor' && (
          <>
            <div className="space-y-1">
              <label htmlFor="ds-sensor-mode" className={labelClassName}>
                Mode
              </label>
              <select id="ds-sensor-mode" value="listener" disabled className={inputClassName}>
                <option value="listener">Listener</option>
              </select>
              <p className="text-xs text-slate-500">{SENSOR_MODE_HELP}</p>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <label htmlFor="ds-sensor-bind-ip" className={labelClassName}>
                  Bind IP
                </label>
                <input
                  id="ds-sensor-bind-ip"
                  type="text"
                  value={bindIp}
                  onChange={(event) => setBindIp(event.target.value)}
                  className={`font-mono ${inputClassName}`}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor="ds-sensor-bind-port" className={labelClassName}>
                  Bind port
                </label>
                <input
                  id="ds-sensor-bind-port"
                  type="number"
                  value={bindPort}
                  onChange={(event) => setBindPort(event.target.value)}
                  className={inputClassName}
                />
              </div>
            </div>
          </>
        )}

        {kind === 'gps' && (
          <>
            <div className="space-y-1">
              <label htmlFor="ds-mode" className={labelClassName}>
                Mode
              </label>
              <select
                id="ds-mode"
                value={gpsMode}
                onChange={(event) => setGpsMode(event.target.value as FormGpsMode)}
                className={inputClassName}
              >
                <option value="gpsd">gpsd</option>
                <option value="serial">serial</option>
                <option value="manual">manual</option>
              </select>
            </div>

            {gpsMode === 'gpsd' && (
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label htmlFor="ds-host" className={labelClassName}>
                    Host
                  </label>
                  <input
                    id="ds-host"
                    type="text"
                    value={host}
                    onChange={(event) => setHost(event.target.value)}
                    className={`font-mono ${inputClassName}`}
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="ds-port" className={labelClassName}>
                    Port
                  </label>
                  <input
                    id="ds-port"
                    type="number"
                    value={port}
                    onChange={(event) => setPort(event.target.value)}
                    className={inputClassName}
                  />
                </div>
              </div>
            )}

            {gpsMode === 'serial' && (
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label htmlFor="ds-device" className={labelClassName}>
                    Device
                  </label>
                  {devicesLoading && <p className="text-sm text-slate-500">Loading devices…</p>}
                  {!devicesLoading && !devicesErrored && serialHasDevices && (
                    <select
                      id="ds-device"
                      value={device}
                      onChange={(event) => setDevice(event.target.value)}
                      className={`font-mono ${inputClassName}`}
                    >
                      <option value="">Select a device…</option>
                      {serialDevices.map((name) => (
                        <option key={name} value={name}>
                          {name}
                        </option>
                      ))}
                    </select>
                  )}
                  {!devicesLoading && !devicesErrored && !serialHasDevices && (
                    <p className="text-sm text-amber-400">{NO_SERIAL_MESSAGE}</p>
                  )}
                </div>
                <div className="space-y-1">
                  <label htmlFor="ds-baud" className={labelClassName}>
                    Baud
                  </label>
                  {/* Fixed dropdown, not free text — the backend's
                     `validate_data_source` rejects any value outside
                     `ALLOWED_BAUD_RATES`, so the UI only ever offers those. */}
                  <select
                    id="ds-baud"
                    value={baud}
                    onChange={(event) => setBaud(Number(event.target.value) as BaudRate)}
                    className={inputClassName}
                  >
                    {BAUD_RATES.map((rate) => (
                      <option key={rate} value={rate}>
                        {rate}
                      </option>
                    ))}
                  </select>
                </div>
              </div>
            )}

            {gpsMode === 'manual' && (
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label htmlFor="ds-lat" className={labelClassName}>
                    Latitude
                  </label>
                  <input
                    id="ds-lat"
                    type="number"
                    step="any"
                    inputMode="decimal"
                    placeholder="37.7749"
                    value={manualLat}
                    onChange={(event) => setManualLat(event.target.value)}
                    className={`font-mono ${inputClassName}`}
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="ds-lon" className={labelClassName}>
                    Longitude
                  </label>
                  <input
                    id="ds-lon"
                    type="number"
                    step="any"
                    inputMode="decimal"
                    placeholder="-122.4194"
                    value={manualLon}
                    onChange={(event) => setManualLon(event.target.value)}
                    className={`font-mono ${inputClassName}`}
                  />
                </div>
              </div>
            )}
          </>
        )}

        {errorMessage && (
          <p role="alert" className="text-sm text-red-400">
            {errorMessage}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={submitting || !canSubmit}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {submitting ? 'Adding…' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
}

interface EditManualModalProps {
  source: DataSource;
  onClose: () => void;
  onSave: (lat: number, lon: number) => void;
  saving: boolean;
}

/** Edit affordance for a stopped `gps`+`manual` source (Task 8): a small
 * lat/lon form that PATCHes the source's static location via
 * `updateDataSource`. Mirrors the Add form's manual lat/lon inputs
 * (`inputClassName`/`labelClassName`) and validation range so the two entry
 * points stay visually and behaviorally consistent. */
function EditManualModal({ source, onClose, onSave, saving }: EditManualModalProps) {
  const cfg = source.config as { lat?: number; lon?: number };
  const [lat, setLat] = useState(cfg.lat != null ? String(cfg.lat) : '');
  const [lon, setLon] = useState(cfg.lon != null ? String(cfg.lon) : '');

  const latNum = Number(lat);
  const lonNum = Number(lon);
  const valid =
    lat.trim() !== '' &&
    lon.trim() !== '' &&
    Number.isFinite(latNum) &&
    Number.isFinite(lonNum) &&
    latNum >= -90 &&
    latNum <= 90 &&
    lonNum >= -180 &&
    lonNum <= 180;

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (valid) onSave(latNum, lonNum);
        }}
        className="w-full max-w-sm space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">Edit Manual Location</h2>
        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1">
            <label htmlFor="edit-lat" className={labelClassName}>
              Latitude
            </label>
            <input
              id="edit-lat"
              type="number"
              step="any"
              inputMode="decimal"
              value={lat}
              onChange={(event) => setLat(event.target.value)}
              className={`font-mono ${inputClassName}`}
            />
          </div>
          <div className="space-y-1">
            <label htmlFor="edit-lon" className={labelClassName}>
              Longitude
            </label>
            <input
              id="edit-lon"
              type="number"
              step="any"
              inputMode="decimal"
              value={lon}
              onChange={(event) => setLon(event.target.value)}
              className={`font-mono ${inputClassName}`}
            />
          </div>
        </div>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={!valid || saving}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
    </div>
  );
}

export default function DataSources() {
  const queryClient = useQueryClient();
  const [showAddForm, setShowAddForm] = useState(false);
  const [editing, setEditing] = useState<DataSource | null>(null);

  const sourcesQuery = useQuery({
    queryKey: queryKeys.dataSources,
    queryFn: listDataSources,
    refetchInterval: REFETCH_INTERVAL_MS,
  });

  function invalidate(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.dataSources });
  }

  const createMutation = useMutation({
    mutationFn: createDataSource,
    onSuccess: () => {
      invalidate();
      setShowAddForm(false);
    },
  });

  const startMutation = useMutation({
    mutationFn: startDataSource,
    onSuccess: invalidate,
  });

  const stopMutation = useMutation({
    mutationFn: stopDataSource,
    onSuccess: invalidate,
  });

  const deleteMutation = useMutation({
    mutationFn: deleteDataSource,
    onSuccess: invalidate,
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, lat, lon }: { id: string; lat: number; lon: number }) =>
      updateDataSource(id, { mode: 'manual', config: { lat, lon } }),
    onSuccess: () => {
      invalidate();
      setEditing(null);
    },
  });

  function handleDelete(id: string): void {
    if (!window.confirm('Delete this data source?')) return;
    deleteMutation.mutate(id);
  }

  const createErrorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create data source.'
        : null;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Data Sources</h1>
        <button
          type="button"
          onClick={() => setShowAddForm(true)}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400"
        >
          Add Data Source
        </button>
      </div>

      {sourcesQuery.isLoading && <p className="text-sm text-slate-500">Loading data sources…</p>}
      {sourcesQuery.isError && <p className="text-sm text-red-400">Failed to load data sources.</p>}

      {sourcesQuery.data && sourcesQuery.data.length === 0 && (
        <p className="text-sm text-slate-500">No data sources configured yet.</p>
      )}

      {sourcesQuery.data && sourcesQuery.data.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Kind</th>
              <th className="py-2 pr-4 font-medium">Mode</th>
              <th className="py-2 pr-4 font-medium">Interface / Config</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 pr-4 font-medium">Health</th>
              <th className="py-2 pr-4 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {sourcesQuery.data.map((source) => {
              const canStart = source.status === 'stopped' || source.status === 'error';
              const canStop = source.status === 'running' || source.status === 'starting';
              const startPending = startMutation.isPending && startMutation.variables === source.id;
              const stopPending = stopMutation.isPending && stopMutation.variables === source.id;
              const deletePending = deleteMutation.isPending && deleteMutation.variables === source.id;
              const updatePending =
                updateMutation.isPending && updateMutation.variables?.id === source.id;
              const rowBusy = startPending || stopPending || deletePending || updatePending;

              return (
                <tr
                  key={source.id}
                  data-testid={`source-row-${source.id}`}
                  className="border-b border-slate-900 align-top"
                >
                  <td className="py-2 pr-4 capitalize text-slate-200">{source.kind}</td>
                  <td className="py-2 pr-4 text-slate-400">{source.mode}</td>
                  <td className="py-2 pr-4">
                    <ConfigSummary source={source} />
                  </td>
                  <td className="py-2 pr-4">
                    <StatusBadge status={source.status} />
                    {source.status === 'error' && source.last_error && (
                      <p className="mt-1 max-w-xs text-xs text-red-400">{source.last_error}</p>
                    )}
                  </td>
                  <td className="py-2 pr-4">
                    <HealthCell source={source} />
                  </td>
                  <td className="py-2 pr-4">
                    <div className="flex gap-2">
                      {canStart && (
                        <button
                          type="button"
                          disabled={rowBusy}
                          onClick={() => startMutation.mutate(source.id)}
                          className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {startPending ? 'Starting…' : 'Start'}
                        </button>
                      )}
                      {canStop && (
                        <button
                          type="button"
                          disabled={rowBusy}
                          onClick={() => stopMutation.mutate(source.id)}
                          className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {stopPending ? 'Stopping…' : 'Stop'}
                        </button>
                      )}
                      {source.kind === 'gps' &&
                        source.mode === 'manual' &&
                        source.status !== 'running' && (
                          <button
                            type="button"
                            disabled={rowBusy}
                            onClick={() => setEditing(source)}
                            className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
                          >
                            Edit
                          </button>
                        )}
                      <button
                        type="button"
                        disabled={rowBusy}
                        onClick={() => handleDelete(source.id)}
                        className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {deletePending ? 'Deleting…' : 'Delete'}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      {showAddForm && (
        <AddSourceForm
          onCancel={() => {
            setShowAddForm(false);
            createMutation.reset();
          }}
          onSubmit={(input) => createMutation.mutate(input)}
          submitting={createMutation.isPending}
          errorMessage={createErrorMessage}
        />
      )}

      {editing && (
        <EditManualModal
          source={editing}
          onClose={() => setEditing(null)}
          onSave={(lat, lon) => updateMutation.mutate({ id: editing.id, lat, lon })}
          saving={updateMutation.isPending}
        />
      )}
    </div>
  );
}

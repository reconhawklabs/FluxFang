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
import {
  BAUD_RATES,
  createDataSource,
  deleteDataSource,
  listCaptureDevices,
  listDataSources,
  startDataSource,
  stopDataSource,
} from '../api/dataSources';
import type {
  BaudRate,
  CreateDataSourceInput,
  DataSource,
  DataSourceStatus,
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
  return <span className="text-slate-500">—</span>;
}

type FormKind = 'wifi' | 'gps';
type FormWifiMode = 'monitor' | 'scan';
type FormGpsMode = 'gpsd' | 'serial';

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

interface AddSourceFormProps {
  onCancel: () => void;
  onSubmit: (input: CreateDataSourceInput) => void;
  submitting: boolean;
  errorMessage: string | null;
}

function AddSourceForm({ onCancel, onSubmit, submitting, errorMessage }: AddSourceFormProps) {
  const [kind, setKind] = useState<FormKind>('wifi');
  const [wifiMode, setWifiMode] = useState<FormWifiMode>('monitor');
  const [iface, setIface] = useState('');
  // Phase B (emitter auto-classification design doc's Frontend section):
  // opt-in per source, defaults OFF. Applies to both wifi modes (monitor
  // captures both beacons and probe requests; scan only surfaces APs) —
  // either way the backend's ingest auto-create only fires when this is set.
  const [autoCreateEmitters, setAutoCreateEmitters] = useState(false);
  const [gpsMode, setGpsMode] = useState<FormGpsMode>('gpsd');
  const [host, setHost] = useState('127.0.0.1');
  const [port, setPort] = useState('2947');
  const [device, setDevice] = useState('');
  const [baud, setBaud] = useState<BaudRate>(9600);

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
  const wifiHasDevices = wifiInterfaces.length > 0;
  const serialHasDevices = serialDevices.length > 0;

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

    if (gpsMode === 'gpsd') {
      onSubmit({ kind: 'gps', mode: 'gpsd', config: { host, port: Number(port) } });
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
  } else if (gpsMode === 'serial') {
    canSubmit = !devicesLoading && !devicesErrored && serialDevices.includes(device);
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

export default function DataSources() {
  const queryClient = useQueryClient();
  const [showAddForm, setShowAddForm] = useState(false);

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
              const rowBusy = startPending || stopPending || deletePending;

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
    </div>
  );
}

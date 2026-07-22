// `GET/POST/PATCH/DELETE /api/data-sources[/:id]` + `POST
// /api/data-sources/:id/{start,stop}` (Task 6.2 backend,
// `fluxfang-api::data_sources`). Every handler there returns
// `fluxfang_db::models::DataSource` directly (no bespoke DTO), so `DataSource`
// below mirrors that struct field-for-field.
//
// `config`'s shape depends on `kind`/`mode` (see `capture::validate_data_source`
// for the exact rules this UI must satisfy before submitting):
//   - wifi + monitor  -> top-level `interface` set, `config: {}`
//   - bluetooth + scan -> top-level `interface` set, `config: {
//     auto_create_emitters, active_scan }`
//   - gps  + gpsd     -> `config: { host, port }`
//   - gps  + serial   -> `config: { device, baud }`, `baud` one of
//     `BAUD_RATES` (the backend's `ALLOWED_BAUD_RATES`)
//   - sensor + listener -> `config: { bind_ip, bind_port,
//     enrollment_window_secs }`, no top-level `interface`
import { del, get, patch, post } from "./client";

// "sensor" is not a kind the Add-Data-Source form creates directly — it's
// the backend-managed listener datasource a distributed Sensor node
// registers against (`fluxfang-api::data_sources`/`sensor_listener`,
// `source.kind == "sensor"`). Included here so pages that read the general
// datasource list (e.g. the Sensors page's "Allow new Sensors" gating) can
// narrow on it without an unsound comparison.
export type DataSourceKind = "wifi" | "gps" | "bluetooth" | "rtl_sdr" | "sensor";
export type DataSourceMode = "monitor" | "scan" | "gpsd" | "serial" | "tpms" | "manual" | "listener";
export type DataSourceStatus = "stopped" | "starting" | "running" | "error";

/** The backend's `ALLOWED_BAUD_RATES` (`fluxfang-api::capture`) — the only
 * baud values `validate_data_source` accepts for a `serial` gps source.
 * The UI presents these as a fixed dropdown, never a free-text field, so an
 * invalid baud can't be typed in the first place. */
export const BAUD_RATES = [4800, 9600, 19200, 38400, 57600, 115200] as const;
export type BaudRate = (typeof BAUD_RATES)[number];

export interface GpsdConfig {
  host: string;
  port: number;
}

export interface SerialConfig {
  device: string;
  baud: BaudRate;
}

/** gps `manual` mode's `config` — an operator-typed static location served as
 * the host position while the source runs. `lat ∈ [-90,90]`, `lon ∈
 * [-180,180]` (validated by the backend's `validate_data_source`). */
export interface ManualConfig {
  lat: number;
  lon: number;
}

/** wifi's `config` — the interface lives on the top-level `interface` field
 * instead (see module doc comment); the only config key wifi sources carry
 * is `auto_create_emitters` (Phase B, emitter auto-classification design
 * doc's Frontend section) — the Add-Source form's "Automatically create
 * emitters" checkbox, wired to backend `data_source.config.auto_create_emitters`
 * (Phase A). Optional/omittable so existing sources without it still narrow. */
export interface WifiConfig {
  auto_create_emitters?: boolean;
}

/** bluetooth's `config` — like wifi, the adapter lives on the top-level
 * `interface` field; `auto_create_emitters` mirrors wifi's checkbox, and
 * `active_scan` toggles active (scan-request) vs. passive BLE advertisement
 * scanning. Both optional/omittable so existing sources without them still
 * narrow. */
export interface BtConfig {
  auto_create_emitters?: boolean;
  active_scan?: boolean;
}

/** The two rtl_433 frequencies the TPMS mode offers (literal rtl_433
 * frequency strings, sent verbatim to the backend and into the command). */
export const RTL_FREQUENCIES = [
  { value: "315M", label: "315 MHz" },
  { value: "433.92M", label: "433.92 MHz" },
] as const;
export type RtlFrequency = (typeof RTL_FREQUENCIES)[number]["value"];

/** rtl_sdr's `config` (mode `tpms`). `device_serial` is the stable USB serial
 * (resolved to rtl_433's `-d :SERIAL` at start); optional for a single-device
 * fallback. `auto_create_emitters` mirrors wifi/bt; `auto_correlate_tpms` is
 * the "Attempt to Connect TPMS Emitters to Other Tires" toggle consumed by
 * Spec B's correlation engine. */
export interface RtlSdrConfig {
  frequency: RtlFrequency;
  device_serial?: string;
  auto_create_emitters?: boolean;
  auto_correlate_tpms?: boolean;
}

/** sensor + listener's `config` — a network listener that accepts
 * connections from distributed Sensor nodes (`fluxfang-api::data_sources`'s
 * `sensor_listener`; validated by the backend's `validate_data_source`:
 * `bind_ip` must be a valid IP, `bind_port` 1..=65535,
 * `enrollment_window_secs` > 0). Unlike wifi/bluetooth/rtl_sdr this kind has
 * no hardware interface — it's a pure network bind, so there's no top-level
 * `interface` field. */
export interface SensorListenerConfig {
  bind_ip: string;
  bind_port: number;
  enrollment_window_secs: number;
}

export type DataSourceConfig =
  | WifiConfig
  | BtConfig
  | RtlSdrConfig
  | GpsdConfig
  | SerialConfig
  | ManualConfig
  | SensorListenerConfig
  | Record<string, never>;

/** Mirrors `fluxfang_db::models::DataSource` exactly. */
export interface DataSource {
  id: string;
  created_at: string;
  kind: DataSourceKind;
  mode: DataSourceMode;
  interface: string | null;
  status: DataSourceStatus;
  config: DataSourceConfig;
  last_error: string | null;
  desired_state: "running" | "stopped";
  last_ok_at: string | null;
}

/** A GPS data source provides *location*, not emissions — so it must not
 * appear anywhere emissions are scoped/filtered by source (dashboard feed
 * tabs, the Emissions page's data-source dropdown, the Map page's Sources
 * group). Everything else (wifi, bluetooth) does emit. */
export function isEmittingSource(source: DataSource): boolean {
  return source.kind !== "gps";
}

/** `POST /api/data-sources` body — mirrors the backend's
 * `CreateDataSourceRequest`. */
export interface CreateDataSourceInput {
  kind: DataSourceKind;
  mode: DataSourceMode;
  interface?: string;
  config: DataSourceConfig;
}

/** `PATCH /api/data-sources/:id` body — mirrors the backend's
 * `UpdateDataSourceRequest` (`kind` is immutable and omitted). */
export interface UpdateDataSourceInput {
  mode: DataSourceMode;
  interface?: string;
  config: DataSourceConfig;
}

export function listDataSources(): Promise<DataSource[]> {
  return get<DataSource[]>("/api/data-sources");
}

export function createDataSource(
  input: CreateDataSourceInput,
): Promise<DataSource> {
  return post<DataSource>("/api/data-sources", input);
}

export function updateDataSource(
  id: string,
  input: UpdateDataSourceInput,
): Promise<DataSource> {
  return patch<DataSource>(`/api/data-sources/${encodeURIComponent(id)}`, input);
}

export function startDataSource(id: string): Promise<DataSource> {
  return post<DataSource>(`/api/data-sources/${encodeURIComponent(id)}/start`);
}

export function stopDataSource(id: string): Promise<DataSource> {
  return post<DataSource>(`/api/data-sources/${encodeURIComponent(id)}/stop`);
}

export function deleteDataSource(id: string): Promise<void> {
  return del<void>(`/api/data-sources/${encodeURIComponent(id)}`);
}

/** One RTL-SDR dongle from `GET /api/system/capture-devices`
 * (`fluxfang_capture::enumerate::RtlSdrDevice`). `serial` is stored in config;
 * `index` is shown for context. */
export interface RtlSdrDevice {
  index: number;
  name: string;
  serial: string;
}

/** `GET /api/system/capture-devices` response (`fluxfang-api::system`) —
 * hardware the Add-Data-Source form enumerates so the user picks an
 * interface/device from a dropdown instead of typing one that may not
 * exist (see `fluxfang_capture::enumerate::{list_wifi_interfaces,
 * list_serial_devices}`). Both arrays can be empty (no matching hardware
 * plugged in), which the form surfaces as a "no compatible device" message
 * rather than falling back to free text. */
export interface CaptureDevices {
  wifi_interfaces: string[];
  serial_devices: string[];
  bluetooth_interfaces: string[];
  rtl_sdr_devices: RtlSdrDevice[];
}

export function listCaptureDevices(): Promise<CaptureDevices> {
  return get<CaptureDevices>("/api/system/capture-devices");
}

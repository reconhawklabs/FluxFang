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
import { del, get, post } from "./client";

export type DataSourceKind = "wifi" | "gps" | "bluetooth";
export type DataSourceMode = "monitor" | "scan" | "gpsd" | "serial";
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

export type DataSourceConfig =
  WifiConfig | BtConfig | GpsdConfig | SerialConfig | Record<string, never>;

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

export function listDataSources(): Promise<DataSource[]> {
  return get<DataSource[]>("/api/data-sources");
}

export function createDataSource(
  input: CreateDataSourceInput,
): Promise<DataSource> {
  return post<DataSource>("/api/data-sources", input);
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
}

export function listCaptureDevices(): Promise<CaptureDevices> {
  return get<CaptureDevices>("/api/system/capture-devices");
}

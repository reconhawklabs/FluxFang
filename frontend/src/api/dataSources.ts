// `GET/POST/PATCH/DELETE /api/data-sources[/:id]` + `POST
// /api/data-sources/:id/{start,stop}` (Task 6.2 backend,
// `fluxfang-api::data_sources`). Every handler there returns
// `fluxfang_db::models::DataSource` directly (no bespoke DTO), so `DataSource`
// below mirrors that struct field-for-field.
//
// `config`'s shape depends on `kind`/`mode` (see `capture::validate_data_source`
// for the exact rules this UI must satisfy before submitting):
//   - wifi + monitor  -> top-level `interface` set, `config: {}`
//   - gps  + gpsd     -> `config: { host, port }`
//   - gps  + serial   -> `config: { device, baud }`, `baud` one of
//     `BAUD_RATES` (the backend's `ALLOWED_BAUD_RATES`)
import { del, get, post } from './client';

export type DataSourceKind = 'wifi' | 'gps';
export type DataSourceMode = 'monitor' | 'gpsd' | 'serial';
export type DataSourceStatus = 'stopped' | 'starting' | 'running' | 'error';

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

/** wifi's `config` is always `{}` — the interface lives on the top-level
 * `interface` field instead (see module doc comment). */
export type DataSourceConfig = GpsdConfig | SerialConfig | Record<string, never>;

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

/** `POST /api/data-sources` body — mirrors the backend's
 * `CreateDataSourceRequest`. */
export interface CreateDataSourceInput {
  kind: DataSourceKind;
  mode: DataSourceMode;
  interface?: string;
  config: DataSourceConfig;
}

export function listDataSources(): Promise<DataSource[]> {
  return get<DataSource[]>('/api/data-sources');
}

export function createDataSource(input: CreateDataSourceInput): Promise<DataSource> {
  return post<DataSource>('/api/data-sources', input);
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

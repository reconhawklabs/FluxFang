// `GET /api/sensors` + `POST /api/sensors/:id/{approve,reject,revoke,rotate}`
// and `POST /api/data-sources/:id/allow-sensors` (Phase 3A backend,
// `fluxfang-api::sensors`/`data_sources`). `GET /api/sensors` returns a
// key-omitting DTO; only `rotate` ever returns a key (shown once).
import { get, post } from './client';

export type SensorStatus = 'pending' | 'approved' | 'revoked' | 'rejected';

export interface Sensor {
  id: string;
  data_source_id: string;
  sensor_id: string;
  fingerprint: string;
  status: SensorStatus;
  auto_group_emitters: boolean;
  source_ip: string | null;
  approved_at: string | null;
  last_seen_at: string | null;
  online: boolean;
}

/** `POST /api/sensors/:id/rotate` response — the freshly generated key,
 * returned exactly once for re-provisioning the sensor node. */
export interface RotatedKey {
  key: string;
  fingerprint: string;
}

export function listSensors(): Promise<Sensor[]> {
  return get<Sensor[]>('/api/sensors');
}

export function approveSensor(id: string, autoGroupEmitters: boolean): Promise<Sensor> {
  return post<Sensor>(`/api/sensors/${id}/approve`, { auto_group_emitters: autoGroupEmitters });
}

export function rejectSensor(id: string): Promise<Sensor> {
  return post<Sensor>(`/api/sensors/${id}/reject`);
}

export function revokeSensor(id: string): Promise<Sensor> {
  return post<Sensor>(`/api/sensors/${id}/revoke`);
}

export function rotateSensor(id: string): Promise<RotatedKey> {
  return post<RotatedKey>(`/api/sensors/${id}/rotate`);
}

export function allowSensors(dataSourceId: string): Promise<{ remaining_secs: number }> {
  return post<{ remaining_secs: number }>(`/api/data-sources/${dataSourceId}/allow-sensors`);
}

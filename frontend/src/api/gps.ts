// `GET /api/gps/status` (Phase 5, `fluxfang-api::gps_status`) — the
// Dashboard GPS Status block's data source, and also what `MapView` reads
// to center the map on the user's current fix. Mirrors `GpsStatusDto`
// field-for-field; see that module's doc comment for how `status` is
// derived (disabled/acquiring/active/degraded).
import { get } from './client';

export type GpsSourceStatus = 'disabled' | 'acquiring' | 'active' | 'degraded';

export interface GpsStatus {
  source_running: boolean;
  has_fix: boolean;
  lat: number | null;
  lon: number | null;
  quality: number | null;
  fix_age_seconds: number | null;
  status: GpsSourceStatus;
}

export function getGpsStatus(): Promise<GpsStatus> {
  return get<GpsStatus>('/api/gps/status');
}

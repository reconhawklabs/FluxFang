// `GET /api/zones` (Task 6.7 backend, `fluxfang-api::zones`). Only the list
// call: Task 9.6's Entities page uses it to populate the zone dropdown for
// an `enters_zone`/`leaves_zone` alert-rule trigger. Full zone
// management (create/edit/delete) belongs to the dedicated Zones page
// (Task 9.8) — YAGNI here.
import { get } from './client';

/** Mirrors `fluxfang-api::dto::ZoneDto`. */
export interface Zone {
  id: string;
  name: string;
  lon: number;
  lat: number;
  radius_m: number;
  notes: string | null;
  created_at: string;
}

export function listZones(): Promise<Zone[]> {
  return get<Zone[]>('/api/zones');
}

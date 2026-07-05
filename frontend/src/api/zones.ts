// `GET/POST/PATCH/DELETE /api/zones[/:id]` (Task 6.7 backend,
// `fluxfang-api::zones`). `listZones` originally only served Task 9.6's
// Entities page (the `enters_zone`/`leaves_zone` zone dropdown) — Task 9.8's
// Zones page grows this module with full management: `createZone`,
// `getZoneDetail` (the subjects — emitters/entities — currently in the
// zone), `patchZone`, `deleteZone`.
import type { Emitter } from './emitters';
import type { Entity } from './entities';
import { del, get, patch, post } from './client';

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

/** `POST`/`PATCH /api/zones[/:id]`'s request-side `center` shape — nested
 * `{lon, lat}`, unlike the flattened `Zone` response (see backend
 * `zones.rs`'s module doc comment for why the two shapes differ). */
export interface ZoneCenter {
  lon: number;
  lat: number;
}

/** `POST /api/zones` body — mirrors the backend's `CreateZoneRequest`. */
export interface CreateZoneInput {
  name: string;
  center: ZoneCenter;
  radius_m: number;
  notes?: string;
}

/** `PATCH /api/zones/:id` body — mirrors the backend's `UpdateZoneRequest`.
 * This page always sends the full form (name/center/radius_m/notes)
 * rather than a partial patch, so every field is required here even though
 * the backend itself treats them as independently optional. */
export interface PatchZoneInput {
  name: string;
  center: ZoneCenter;
  radius_m: number;
  notes?: string;
}

/** `GET /api/zones/:id`'s response — mirrors the backend's `ZoneDetailDto`
 * (every `Zone` field flattened, plus the subjects — emitters/entities —
 * currently "in" the zone per `ZoneRepo::subjects_in_zone`). */
export interface ZoneDetail extends Zone {
  emitters: Emitter[];
  entities: Entity[];
}

export function listZones(): Promise<Zone[]> {
  return get<Zone[]>('/api/zones');
}

export function createZone(input: CreateZoneInput): Promise<Zone> {
  return post<Zone>('/api/zones', input);
}

export function getZoneDetail(id: string): Promise<ZoneDetail> {
  return get<ZoneDetail>(`/api/zones/${encodeURIComponent(id)}`);
}

export function patchZone(id: string, body: PatchZoneInput): Promise<Zone> {
  return patch<Zone>(`/api/zones/${encodeURIComponent(id)}`, body);
}

export function deleteZone(id: string): Promise<void> {
  return del<void>(`/api/zones/${encodeURIComponent(id)}`);
}

// `GET/PATCH/DELETE /api/entities[/:id]` + `POST /api/entities` (Task 6.5
// backend, `fluxfang-api::entities`). Originally only carried the two calls
// Task 9.5's Emitters page needed (list + create — populating the
// "associate to existing entity" dropdown and the first half of "create
// entity & associate"). Task 9.6's Entities page grows this with the
// detail fetch (`getEntityDetail`) plus `patchEntity`/`deleteEntity` for
// editing/removing an entity — still YAGNI beyond that.
import type { Emitter } from './emitters';
import { del, get, patch, post } from './client';

/** Mirrors `fluxfang-api::dto::EntityDto`. */
export interface Entity {
  id: string;
  name: string;
  notes: string | null;
  created_at: string;
}

/** One row in `GET /api/entities/:id`'s `recent_detections` — mirrors the
 * backend's `RecentDetectionDto`. */
export interface RecentDetection {
  emitter_id: string | null;
  lat: number;
  lon: number;
  observed_at: string;
}

/** `GET /api/entities/:id`'s response — mirrors the backend's
 * `EntityDetailDto` (every `Entity` field flattened, plus `last_seen`,
 * `emitters`, `recent_detections`). */
export interface EntityDetail extends Entity {
  last_seen: string | null;
  emitters: Emitter[];
  recent_detections: RecentDetection[];
}

/** `POST /api/entities` body — mirrors the backend's `CreateEntityRequest`. */
export interface CreateEntityInput {
  name: string;
  notes?: string;
}

/** `PATCH /api/entities/:id` body — mirrors the backend's
 * `UpdateEntityRequest`. Both fields optional/independently omittable
 * (`notes: null` clears it, an omitted `notes` leaves it alone), same
 * "distinguish absent from explicit null" convention as
 * `PatchEmitterInput`. */
export interface PatchEntityInput {
  name?: string;
  notes?: string | null;
}

/** `GET /api/entities` query params — mirrors the backend's
 * `ListEntitiesQuery`. All optional; a caller that only wants the full
 * (interim, pre-pagination) set passes just `{ limit: 500 }`, same
 * convention as `listEmitters`'s params object. */
export interface ListEntitiesParams {
  search?: string;
  limit?: number;
  offset?: number;
}

/** `GET /api/entities`'s response envelope — mirrors the backend's
 * `EntitiesPageDto`. */
export interface EntitiesPage {
  items: Entity[];
  total: number;
}

export function listEntities(params: ListEntitiesParams = {}): Promise<EntitiesPage> {
  const query = new URLSearchParams();
  if (params.search !== undefined) query.set('search', params.search);
  if (params.limit !== undefined) query.set('limit', String(params.limit));
  if (params.offset !== undefined) query.set('offset', String(params.offset));
  const qs = query.toString();
  return get<EntitiesPage>(`/api/entities${qs.length > 0 ? `?${qs}` : ''}`);
}

export function getEntityDetail(id: string): Promise<EntityDetail> {
  return get<EntityDetail>(`/api/entities/${encodeURIComponent(id)}`);
}

export function createEntity(input: CreateEntityInput): Promise<Entity> {
  return post<Entity>('/api/entities', input);
}

export function patchEntity(id: string, body: PatchEntityInput): Promise<Entity> {
  return patch<Entity>(`/api/entities/${encodeURIComponent(id)}`, body);
}

export function deleteEntity(id: string): Promise<void> {
  return del<void>(`/api/entities/${encodeURIComponent(id)}`);
}

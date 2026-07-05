// `GET /api/entities` + `POST /api/entities` (Task 6.5 backend,
// `fluxfang-api::entities`). Only the two calls Task 9.5's Emitters page
// needs: the list (to populate the "associate to existing entity" dropdown
// and resolve an emitter's `entity_id` to a display name) and creating a
// new entity as the first half of "create entity & associate" (the second
// half is `patchEmitter(id, { entity_id })`, see `api/emitters.ts`). A
// dedicated Entities page (Task 9.6) can grow this module with
// update/delete/detail as needed then — YAGNI for this slice.
import { get, post } from './client';

/** Mirrors `fluxfang-api::dto::EntityDto`. */
export interface Entity {
  id: string;
  name: string;
  notes: string | null;
  created_at: string;
}

/** `POST /api/entities` body — mirrors the backend's `CreateEntityRequest`. */
export interface CreateEntityInput {
  name: string;
  notes?: string;
}

export function listEntities(): Promise<Entity[]> {
  return get<Entity[]>('/api/entities');
}

export function createEntity(input: CreateEntityInput): Promise<Entity> {
  return post<Entity>('/api/entities', input);
}

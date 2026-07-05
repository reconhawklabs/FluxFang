// `GET/PATCH/DELETE /api/emitters[/:id]` + `POST /api/emitters` (Task 6.4
// backend, `fluxfang-api::emitters`). Originally only carried the two calls
// Task 9.4's Emissions page needed (list + create); Task 9.5's Emitters page
// grows this with `patchEmitter` (rename / associate-to-entity / detach) and
// `deleteEmitter` — still YAGNI beyond that (no `/rule` or `/preview` calls
// here; `RuleBuilder`/`Emissions.tsx` own those directly).
import type { Rule } from '../types/rule';
import { del, get, patch, post } from './client';

/** Mirrors `fluxfang-api::dto::EmitterDto`. */
export interface Emitter {
  id: string;
  name: string;
  type: string | null;
  entity_id: string | null;
  match_criteria: unknown;
  first_seen_at: string | null;
  last_seen_at: string | null;
  created_at: string;
}

/** `POST /api/emitters` body — mirrors the backend's `CreateEmitterRequest`.
 * This page only ever sends `match_criteria` directly (a `RuleBuilder`
 * rule), not the `from_emission_id` prefill alternative the backend also
 * accepts — the prefill happens client-side instead (see
 * `Emissions.tsx`'s `defaultRuleFor`), so the built rule is visible/editable
 * in the modal before submitting. */
export interface CreateEmitterInput {
  name: string;
  type?: string;
  match_criteria: Rule;
}

/** `POST /api/emitters`'s response envelope — mirrors the backend's
 * `EmitterAndCount`. */
export interface CreateEmitterResult {
  emitter: Emitter;
  attached_count: number;
}

/** `PATCH /api/emitters/:id` body — mirrors the backend's
 * `UpdateEmitterRequest`. Every field is optional and independently
 * omittable: `entity_id` in particular distinguishes "key absent" (leave
 * alone) from `entity_id: null` (detach) from `entity_id: "<uuid>"`
 * (associate) — see that struct's `some` deserializer doc comment. Callers
 * here only ever send the keys they mean to change (e.g. `{ entity_id:
 * null }` to detach, never a full object), so that distinction is
 * preserved on the wire. */
export interface PatchEmitterInput {
  name?: string;
  type?: string | null;
  entity_id?: string | null;
}

export function listEmitters(): Promise<Emitter[]> {
  return get<Emitter[]>('/api/emitters');
}

export function createEmitter(input: CreateEmitterInput): Promise<CreateEmitterResult> {
  return post<CreateEmitterResult>('/api/emitters', input);
}

export function patchEmitter(id: string, body: PatchEmitterInput): Promise<Emitter> {
  return patch<Emitter>(`/api/emitters/${id}`, body);
}

export function deleteEmitter(id: string): Promise<void> {
  return del<void>(`/api/emitters/${id}`);
}

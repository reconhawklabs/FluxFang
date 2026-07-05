// `GET /api/emitters` + `POST /api/emitters` (Task 6.4 backend,
// `fluxfang-api::emitters`). Only the two calls Task 9.4's Emissions page
// needs: the list (to resolve an emission's `emitter_id` to a display name)
// and creating a new emitter from a `RuleBuilder` rule ("assign to
// emitter"). A dedicated Emitters page (Task 9.5) can grow this module with
// update/delete/preview/etc. as needed then — YAGNI for this slice.
import type { Rule } from '../types/rule';
import { get, post } from './client';

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

export function listEmitters(): Promise<Emitter[]> {
  return get<Emitter[]>('/api/emitters');
}

export function createEmitter(input: CreateEmitterInput): Promise<CreateEmitterResult> {
  return post<CreateEmitterResult>('/api/emitters', input);
}

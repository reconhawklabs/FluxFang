// `GET/PATCH/DELETE /api/emitters[/:id]` + `POST /api/emitters` (Task 6.4
// backend, `fluxfang-api::emitters`). Originally only carried the two calls
// Task 9.4's Emissions page needed (list + create); Task 9.5's Emitters page
// grows this with `patchEmitter` (rename / associate-to-entity / detach) and
// `deleteEmitter`. Phase B (emitter auto-classification design doc) adds the
// classification fields (`emitter_type`/`attributes`/`match_enabled` +
// derived `type_label`/`category`) and the matching `PatchEmitterInput`
// fields — still YAGNI beyond that (no `/rule` or `/preview` calls here;
// `RuleBuilder`/`Emissions.tsx` own those directly).
import type { Rule } from '../types/rule';
import { del, get, patch, post } from './client';

/** Type-specific identifying info + metadata an auto-classified emitter
 * carries (Phase A backend's `emitter.attributes jsonb`) — e.g.
 * `{ssid, bssid}` for a `wifi_access_point`, `{src_mac, randomized_mac}` for
 * a `wifi_client`. Left as a loose record (not a per-type union) since a
 * plain user-made emitter has `{}` and future device kinds (bluetooth,
 * sensors, …) add their own keys with no schema change — callers read
 * specific keys out of it defensively, same convention as
 * `Emission.payload`. */
export type EmitterAttributes = Record<string, unknown>;

/** Mirrors `fluxfang-api::dto::EmitterDto`. */
export interface Emitter {
  id: string;
  name: string;
  type: string | null;
  /** Machine classification key, e.g. `"wifi_access_point"` /
   * `"wifi_client"` — `null` for a plain user-made emitter. */
  emitter_type: string | null;
  attributes: EmitterAttributes;
  /** Whether this emitter's `match_criteria` rule auto-attaches new
   * matching emissions. Toggled via `PATCH { match_enabled }`; when
   * `false`, ingest leaves newly-matching emissions unassigned instead
   * (and, for an auto-create rule, won't re-create the emitter either). */
  match_enabled: boolean;
  /** Derived human label for `emitter_type`, e.g. "WiFi Access Point" —
   * `null` when `emitter_type` is `null`. */
  type_label: string | null;
  /** Derived grouping key for `emitter_type`, e.g. `"wifi"` — `null` when
   * `emitter_type` is `null`. Reserved for the Phase C map's category-layer
   * filter; not used by this phase's pages. */
  category: string | null;
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
  /** Machine classification key (e.g. `"wifi_access_point"`) — a valid
   * option from `listEmitterTypes(kind)`. Omit (don't send `null`) for a
   * free-text/custom type, which leaves the backend's `emitter_type` column
   * `null` (see `fluxfang-api::emitters`'s `CreateEmitterRequest`). An
   * invalid key 400s. */
  emitter_type?: string;
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
 * preserved on the wire.
 *
 * `attributes` is a **full replace**, not a merge (per the design doc's API
 * section) — the backend simply overwrites `emitter.attributes` with
 * whatever's sent. A caller that wants to flip one key (e.g. the manual
 * `randomized_mac` override, `Emitters.tsx`'s `handleToggleRandomized`)
 * must read the emitter's current `attributes`, spread it, and override
 * just that key before sending — never send a partial object. */
export interface PatchEmitterInput {
  name?: string;
  type?: string | null;
  entity_id?: string | null;
  match_enabled?: boolean;
  attributes?: EmitterAttributes;
}

/** One valid `emitter_type` choice for a given emission `kind` — `key` is
 * the machine classification value sent as `CreateEmitterInput.emitter_type`
 * / `PatchEmitterInput`'s equivalent, `label` the human-readable name (also
 * sent as the free-text `type` alongside it, so `Emitter.type` stays
 * populated for older UI that only reads `type`, not `emitter_type`). */
export interface EmitterType {
  key: string;
  label: string;
}

/** `GET /api/emitter-types/:kind` — the valid `emitter_type` keys/labels for
 * an emission kind's data source (e.g. `"wifi"` → wifi access point/client),
 * used by the Emissions "Assign to emitter" modal's Type dropdown. Empty
 * array for an unknown kind. */
export function listEmitterTypes(kind: string): Promise<EmitterType[]> {
  return get<EmitterType[]>(`/api/emitter-types/${encodeURIComponent(kind)}`);
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

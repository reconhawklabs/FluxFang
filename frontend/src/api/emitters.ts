// `GET/PATCH/DELETE /api/emitters[/:id]` + `POST /api/emitters` (Task 6.4
// backend, `fluxfang-api::emitters`). Originally only carried the two calls
// Task 9.4's Emissions page needed (list + create); Task 9.5's Emitters page
// grows this with `patchEmitter` (rename / associate-to-entity / detach) and
// `deleteEmitter`. Phase B (emitter auto-classification design doc) adds the
// classification fields (`emitter_type`/`attributes`/`match_enabled` +
// derived `type_label`/`category`) and the matching `PatchEmitterInput`
// fields. The Emitters page's expanded-row rule editor (list-pages UX
// cleanup) adds `setEmitterRule` (`POST /api/emitters/:id/rule`); the
// `/preview` call still lives in `RuleBuilder` directly.
import type { Rule } from "../types/rule";
import { del, get, patch, post } from "./client";

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
  /** Count of emissions currently attached to this emitter — backs the
   * Emitters page's "Emissions" column. */
  emission_count: number;
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

/** `GET /api/emitters` query params — mirrors the backend's
 * `ListEmittersQuery`. All optional; a caller that only wants the full
 * (interim, pre-pagination) set passes just `{ limit: 500 }`, same
 * convention as `listNotifications`'s params object. */
export interface ListEmittersParams {
  search?: string;
  entity_id?: string;
  /** Exact-match filter on the emitter's machine `emitter_type` key (e.g.
   * `"wifi_access_point"`), backing the Emitters page's Type-filter
   * dropdown. ANDed with `search`/`entity_id` server-side. */
  emitter_type?: string;
  limit?: number;
  offset?: number;
  sort?: string;
  dir?: "asc" | "desc";
}

/** `GET /api/emitters`'s response envelope — mirrors the backend's
 * `EmittersPageDto`. */
export interface EmittersPage {
  items: Emitter[];
  total: number;
}

/** `GET /api/emitters/types` — the distinct `emitter_type` values that
 * actually have at least one emitter, each with its machine `key` and
 * human-readable `label`, sorted by label. Backs the Emitters page's
 * Type-filter dropdown with a stable option set, instead of deriving
 * options from whatever rows happen to be currently loaded/paginated. */
export function listEmitterTypesInUse(): Promise<EmitterType[]> {
  return get<EmitterType[]>("/api/emitters/types");
}

export function listEmitters(
  params: ListEmittersParams = {},
): Promise<EmittersPage> {
  const query = new URLSearchParams();
  if (params.search !== undefined) query.set("search", params.search);
  if (params.entity_id !== undefined) query.set("entity_id", params.entity_id);
  if (params.emitter_type !== undefined)
    query.set("emitter_type", params.emitter_type);
  if (params.limit !== undefined) query.set("limit", String(params.limit));
  if (params.offset !== undefined) query.set("offset", String(params.offset));
  if (params.sort !== undefined) query.set("sort", params.sort);
  if (params.dir !== undefined) query.set("dir", params.dir);
  const qs = query.toString();
  return get<EmittersPage>(`/api/emitters${qs.length > 0 ? `?${qs}` : ""}`);
}

export function createEmitter(
  input: CreateEmitterInput,
): Promise<CreateEmitterResult> {
  return post<CreateEmitterResult>("/api/emitters", input);
}

export function patchEmitter(
  id: string,
  body: PatchEmitterInput,
): Promise<Emitter> {
  return patch<Emitter>(`/api/emitters/${id}`, body);
}

/** `GET /api/emitters/:id` — a single emitter by id (backend handler
 * `get_emitter`), backing the emitter detail page. */
export function getEmitter(id: string): Promise<Emitter> {
  return get<Emitter>(`/api/emitters/${encodeURIComponent(id)}`);
}

/** `POST /api/emitters/:id/rule { match_criteria }` — replace an existing
 * emitter's match rule (the Emitters page's expanded-row rule editor). The
 * backend validates the rule, persists it, then re-attaches every already-
 * stored emission that now matches, returning the updated emitter plus that
 * `attached_count` (same `EmitterAndCount` envelope as `POST /api/emitters`,
 * hence the shared `CreateEmitterResult` return type). */
export function setEmitterRule(
  id: string,
  rule: Rule,
): Promise<CreateEmitterResult> {
  return post<CreateEmitterResult>(`/api/emitters/${id}/rule`, {
    match_criteria: rule,
  });
}

export function deleteEmitter(id: string): Promise<void> {
  return del<void>(`/api/emitters/${id}`);
}

/** Shared response shape for both endpoints below (mirrors
 * `fluxfang-api::emitters`'s `DeletedCountDto` — same shape
 * `api/emissions.ts`'s own `DeletedCount` mirrors for its bulk endpoints). */
export interface DeletedCount {
  deleted: number;
}

/** `POST /api/emitters/bulk-delete {ids}` — the Emitters page's mass-select
 * "Delete selected" action (Phase 3, `SelectionToolbar`). A `POST` to a
 * dedicated path rather than `DELETE` with a body, same convention as
 * `bulkDeleteEmissions`. */
export function bulkDeleteEmitters(ids: string[]): Promise<DeletedCount> {
  return post<DeletedCount>("/api/emitters/bulk-delete", { ids });
}

/** `POST /api/emitters/clear` (no body) — "Clear All Emitters", gated by
 * `SelectionToolbar`'s confirm dialog before this is ever called. */
export function clearEmitters(): Promise<DeletedCount> {
  return post<DeletedCount>("/api/emitters/clear");
}

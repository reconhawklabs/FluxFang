// `GET /api/emissions` (Task 6.3 backend, `fluxfang-api::emissions`). Mirrors
// `EmissionDto` field-for-field (`crates/fluxfang-api/src/dto.rs`) — see that
// struct's doc comment for why `lon`/`lat` are `Option<f64>` (projected from
// PostGIS, `None` when the emission has no location).
//
// Query params are built by the caller (Task 9.4's Emissions page) via
// `components/filterState.ts`'s `filterToQueryParams` (field-condition/text/
// unassigned filters) merged with this module's own pagination params —
// see `listEmissions`'s doc comment.
import { get, post } from './client';

/** Mirrors `fluxfang-api::dto::EmissionDto`. `payload`'s shape depends on
 * `kind` (for `"wifi"`: `bssid`/`ssid`/`channel`, per
 * `fluxfang-core::catalog`'s wifi field list) — left as `Record<string,
 * unknown>` rather than a typed union since this page only ever reads a few
 * known-optional keys out of it defensively. Phase A's parser accuracy fix
 * adds `src_mac` (the probing client's MAC) alongside `bssid` (the AP) —
 * beacons populate `bssid` and leave `src_mac` absent; probe requests
 * populate `src_mac` and leave `bssid` absent (see the emitter
 * auto-classification design doc's "Parser accuracy fix" section). */
export interface Emission {
  id: string;
  data_source_id: string | null;
  emitter_id: string | null;
  session_id: string | null;
  observed_at: string;
  signal_strength: number | null;
  lon: number | null;
  lat: number | null;
  kind: string;
  payload: Record<string, unknown>;
}

/** `GET /api/emissions`'s response envelope. */
export interface EmissionsPage {
  items: Emission[];
  total: number;
}

/**
 * `GET /api/emissions?<params>`. `params` is a plain `URLSearchParams` the
 * caller has already built (typically `filterToQueryParams(filterState)`
 * with `limit`/`offset` appended) — this function doesn't interpret or
 * validate it, it's a direct passthrough to the backend's `RawQuery`-based
 * `parse_filter` (see that module's doc comment for the full param list:
 * `q`, repeated `cond`, `unassigned`, `data_source_id`, `emitter_id`,
 * `time_from`/`time_to`, `bbox`, `kind`, `match`, `limit`, `offset`).
 */
export function listEmissions(params: URLSearchParams): Promise<EmissionsPage> {
  const qs = params.toString();
  return get<EmissionsPage>(`/api/emissions${qs.length > 0 ? `?${qs}` : ''}`);
}

/** Shared response shape for both endpoints below (`fluxfang-api::emissions`'s
 * `DeletedCountDto`). */
export interface DeletedCount {
  deleted: number;
}

/**
 * `POST /api/emissions/bulk-delete {ids}` — the Emissions page's mass-select
 * "Delete selected" action (Phase 2, `SelectionToolbar`). A `POST` to a
 * dedicated path rather than `DELETE` with a body — see that route's doc
 * comment for why (some proxies strip `DELETE` bodies).
 */
export function bulkDeleteEmissions(ids: string[]): Promise<DeletedCount> {
  return post<DeletedCount>('/api/emissions/bulk-delete', { ids });
}

/** `POST /api/emissions/clear` (no body) — "Clear All Emissions", gated by
 * `SelectionToolbar`'s confirm dialog before this is ever called. */
export function clearEmissions(): Promise<DeletedCount> {
  return post<DeletedCount>('/api/emissions/clear');
}

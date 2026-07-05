// `GET /api/emissions` (Task 6.3 backend, `fluxfang-api::emissions`). Mirrors
// `EmissionDto` field-for-field (`crates/fluxfang-api/src/dto.rs`) — see that
// struct's doc comment for why `lon`/`lat` are `Option<f64>` (projected from
// PostGIS, `None` when the emission has no location).
//
// Query params are built by the caller (Task 9.4's Emissions page) via
// `components/filterState.ts`'s `filterToQueryParams` (field-condition/text/
// unassigned filters) merged with this module's own pagination params —
// see `listEmissions`'s doc comment.
import { get } from './client';

/** Mirrors `fluxfang-api::dto::EmissionDto`. `payload`'s shape depends on
 * `kind` (for `"wifi"`: `bssid`/`ssid`/`channel`, per
 * `fluxfang-core::catalog`'s wifi field list) — left as `Record<string,
 * unknown>` rather than a typed union since this page only ever reads a few
 * known-optional keys out of it defensively. */
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

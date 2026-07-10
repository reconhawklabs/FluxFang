// `GET /api/catalog/:kind` (Task 6.1 backend) as a TanStack Query hook.
// Shared by `RuleBuilder` and `FilterBar` (Task 9.2) so both build their
// field/operator dropdowns off the exact same data and query cache entry —
// see `queryKeys.catalog(kind)`'s doc comment for why this key is never
// invalidated by `useLiveEvents` (the catalog is static server config, not
// live data).
//
// Task 4 (frontend attribute filter on the Emitters page) generalizes this
// to also serve `GET /api/emitter-catalog/:kind` — the emitter attribute
// catalog for an `emitter_type` (Tasks 1-3 backend) — via the optional
// `source` param, so `StackedFilterBuilder` can point at either catalog
// without duplicating this fetch/cache-key wiring. `source` defaults to
// `"datasource"` so the existing `RuleBuilder`/`FilterBar`/Emissions-page
// call sites (`useCatalog(kind)`, no second arg) are unaffected.
import { useQuery } from '@tanstack/react-query';
import { get } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import type { FieldDef } from '../types/catalog';

export function useCatalog(
  kind: string,
  source: 'datasource' | 'emitter' = 'datasource',
) {
  return useQuery({
    queryKey: source === 'emitter' ? ['emitterCatalog', kind] : queryKeys.catalog(kind),
    queryFn: () =>
      get<FieldDef[]>(
        source === 'emitter'
          ? `/api/emitter-catalog/${encodeURIComponent(kind)}`
          : `/api/catalog/${encodeURIComponent(kind)}`,
      ),
    // The catalog is fixed server-side config for a given `kind` — no point
    // refetching it every time a component using it remounts.
    staleTime: Infinity,
    // Emitters.tsx passes `kind = emitterType`, which is `""` (the "All
    // types" sentinel) until a specific type is picked — skip the fetch
    // rather than hitting `/api/emitter-catalog/` with an empty segment.
    enabled: kind.length > 0,
  });
}

// `GET /api/catalog/:kind` (Task 6.1 backend) as a TanStack Query hook.
// Shared by `RuleBuilder` and `FilterBar` (Task 9.2) so both build their
// field/operator dropdowns off the exact same data and query cache entry —
// see `queryKeys.catalog(kind)`'s doc comment for why this key is never
// invalidated by `useLiveEvents` (the catalog is static server config, not
// live data).
import { useQuery } from '@tanstack/react-query';
import { get } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import type { FieldDef } from '../types/catalog';

export function useCatalog(kind: string) {
  return useQuery({
    queryKey: queryKeys.catalog(kind),
    queryFn: () => get<FieldDef[]>(`/api/catalog/${encodeURIComponent(kind)}`),
    // The catalog is fixed server-side config for a given `kind` — no point
    // refetching it every time a component using it remounts.
    staleTime: Infinity,
  });
}

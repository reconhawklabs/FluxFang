// Fetches the node's role/config from the protected `GET /api/config`.
// Callers render inside the authed AppShell, so the endpoint is reachable;
// role rarely changes within a session, so the result is treated as fresh.
import { useQuery } from '@tanstack/react-query';
import type { UseQueryResult } from '@tanstack/react-query';
import { api, ApiError } from '../api/client';
import type { AppConfig } from '../api/client';

export function useConfig(enabled = true): UseQueryResult<AppConfig> {
  return useQuery<AppConfig>({
    queryKey: ['config'],
    queryFn: () => api.config(),
    enabled,
    staleTime: Infinity,
    // A 404 is the expected "this install has no node role yet" case (a legacy
    // DB upgraded before its backfill migration ran) — settle immediately and
    // let `App` fall back to Standalone. Any other error is likely transient
    // (network blip, 5xx), so retry a couple of times: because `App` latches on
    // the first settle, a real Sensor node must not get pinned to the wrong
    // (Standalone) role by a single flaky request.
    retry: (failureCount, error) =>
      !(error instanceof ApiError && error.status === 404) && failureCount < 2,
  });
}

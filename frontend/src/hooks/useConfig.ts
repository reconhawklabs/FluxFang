// Fetches the node's role/config from the protected `GET /api/config`.
// Callers render inside the authed AppShell, so the endpoint is reachable;
// role rarely changes within a session, so the result is treated as fresh.
import { useQuery } from '@tanstack/react-query';
import type { UseQueryResult } from '@tanstack/react-query';
import { api } from '../api/client';
import type { AppConfig } from '../api/client';

export function useConfig(enabled = true): UseQueryResult<AppConfig> {
  return useQuery<AppConfig>({
    queryKey: ['config'],
    queryFn: () => api.config(),
    enabled,
    staleTime: Infinity,
    retry: false,
  });
}

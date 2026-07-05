// Derives `{needsSetup, authed, loading}` from two things:
//
// 1. `GET /api/setup/status` (public) → `needs_setup`.
// 2. An "auth probe": a cheap request to a PROTECTED endpoint, to tell
//    apart "setup is done but I'm not logged in" (401) from "I'm logged
//    in" (200). There's no dedicated `/api/me`-style endpoint, so this
//    picks `GET /api/notifications?limit=1` — protected, side-effect-free,
//    and cheap (a single small page query). If a real `/api/me` endpoint
//    is ever added, swap the probe path here; nothing else needs to change.
//
// The probe only runs once setup-status is known and setup is NOT needed
// (no point probing auth before there's even a password to authenticate
// against).
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, get } from '../api/client';

const AUTH_PROBE_PATH = '/api/notifications?limit=1';

export interface UseAuthResult {
  needsSetup: boolean;
  authed: boolean;
  loading: boolean;
  refetch: () => Promise<void>;
}

export function useAuth(): UseAuthResult {
  const queryClient = useQueryClient();

  const setupQuery = useQuery({
    queryKey: ['auth', 'setupStatus'],
    queryFn: () => api.setupStatus(),
    retry: false,
  });

  const needsSetup = setupQuery.data?.needs_setup ?? false;

  const probeQuery = useQuery({
    queryKey: ['auth', 'probe'],
    queryFn: async (): Promise<boolean> => {
      try {
        await get(AUTH_PROBE_PATH);
        return true;
      } catch (err) {
        if (err instanceof ApiError && err.status === 401) return false;
        throw err;
      }
    },
    enabled: setupQuery.isSuccess && !needsSetup,
    retry: false,
  });

  const loading = setupQuery.isLoading || (!needsSetup && probeQuery.isLoading);

  const refetch = async (): Promise<void> => {
    await queryClient.invalidateQueries({ queryKey: ['auth'] });
  };

  return {
    needsSetup,
    authed: probeQuery.data === true,
    loading,
    refetch,
  };
}

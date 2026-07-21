// Polls `GET /api/sensors` so pending registrations and health/online status
// stay current without a WS channel (like the DataSources page's poll).
import { useQuery } from '@tanstack/react-query';
import type { UseQueryResult } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { listSensors } from '../api/sensors';
import type { Sensor } from '../api/sensors';

const REFETCH_INTERVAL_MS = 4000;

export function useSensors(enabled = true): UseQueryResult<Sensor[]> {
  return useQuery<Sensor[]>({
    queryKey: queryKeys.sensors,
    queryFn: listSensors,
    enabled,
    refetchInterval: REFETCH_INTERVAL_MS,
  });
}

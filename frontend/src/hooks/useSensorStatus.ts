// Polls a Sensor node's own forwarding status: `GET /api/sensor/status`
// (cache depth/undelivered backlog + the standalone target it forwards to).
// Backs `SensorDashboard`'s forwarding-status section.
import { useQuery } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { api } from '../api/client';
import type { SensorStatus } from '../api/client';

export function useSensorStatus(enabled = true) {
  return useQuery<SensorStatus>({
    queryKey: queryKeys.sensorStatus,
    queryFn: () => api.sensorStatus(),
    enabled,
    // Every 30s: the endpoint does a live connectivity probe to the Standalone
    // (checked at least once a minute, per the Dashboard's connection metric),
    // and this refreshes cache depth + last-hour throughput.
    refetchInterval: 30000,
  });
}

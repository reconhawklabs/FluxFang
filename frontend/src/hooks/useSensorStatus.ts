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
    refetchInterval: 4000,
  });
}

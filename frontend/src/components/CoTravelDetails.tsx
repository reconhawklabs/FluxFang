// The expanded Co-Travel row content: where and how strongly a device was
// heard within the active window. Lazily (only when a row is expanded)
// fetches that emitter's located detections and renders a points map + an
// RSSI sparkline. Reuses GET /api/emissions — no co-travel-specific endpoint.
import { useQuery } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { listEmissions, type Emission } from '../api/emissions';
import SightingPointsMap from './SightingPointsMap';
import RssiSparkline from './RssiSparkline';

export interface CoTravelDetailsProps {
  emitterId: string;
  from?: string;
  to?: string;
}

export default function CoTravelDetails({ emitterId, from, to }: CoTravelDetailsProps) {
  const query = useQuery({
    queryKey: [...queryKeys.emissions, { emitter_id: emitterId, from, to }],
    queryFn: () => {
      const params = new URLSearchParams();
      params.set('emitter_id', emitterId);
      if (from) params.set('time_from', from);
      if (to) params.set('time_to', to);
      params.set('limit', '500');
      return listEmissions(params);
    },
  });

  if (query.isLoading) {
    return <div className="px-1 py-2 text-xs text-slate-500">Loading detections…</div>;
  }
  if (query.isError) {
    return <div className="px-1 py-2 text-xs text-red-400">Failed to load detections.</div>;
  }

  const items = query.data?.items ?? [];
  const located = items.filter(
    (e): e is Emission & { lon: number; lat: number } => e.lon !== null && e.lat !== null,
  );
  const mapPoints = located.map((e) => ({
    lon: e.lon,
    lat: e.lat,
    signal_strength: e.signal_strength,
  }));
  const rssiPoints = items.map((e) => ({
    observed_at: e.observed_at,
    signal_strength: e.signal_strength,
  }));

  return (
    <div className="space-y-3 rounded border border-slate-800 bg-slate-950/40 p-3">
      <SightingPointsMap points={mapPoints} />
      <div>
        <div className="mb-1 text-xs text-slate-500">Signal strength over time</div>
        <RssiSparkline points={rssiPoints} width={320} />
      </div>
      <div className="text-xs text-slate-500">{located.length} located of {items.length} detections</div>
    </div>
  );
}

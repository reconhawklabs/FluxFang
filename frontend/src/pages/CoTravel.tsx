// Co-Travel Detection: ranks emitters by how strongly they behave like they
// followed you (seen at different places + times), grouped into tiers, with a
// per-row Ignore. Two snap sliders drive both the gate and the score
// resolution server-side (see design doc §4). Defaults to all data; a future
// iteration adds the from/to window inputs.
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import RangeSlider from '../components/RangeSlider';
import { queryKeys } from '../api/queryKeys';
import {
  ignoreEmitter,
  listCoTravel,
  type CoTravelItem,
  type CoTravelTier,
} from '../api/coTravel';
import { useDebouncedValue } from '../hooks/useDebouncedValue';

const DISTANCE_STOPS = [
  { label: '100 ft', meters: 30.48 },
  { label: '500 ft', meters: 152.4 },
  { label: '¼ mi', meters: 402.336 },
  { label: '1 mi', meters: 1609.34 },
  { label: '5 mi', meters: 8046.72 },
  { label: '15 mi', meters: 24140.2 },
  { label: '30 mi', meters: 48280.3 },
  { label: '50 mi', meters: 80467.2 },
  { label: '100 mi', meters: 160934.4 },
] as const;

const TIME_STOPS = [
  { label: '30 s', seconds: 30 },
  { label: '1 m', seconds: 60 },
  { label: '5 m', seconds: 300 },
  { label: '15 m', seconds: 900 },
  { label: '30 m', seconds: 1800 },
  { label: '1 h', seconds: 3600 },
  { label: '4 h', seconds: 14400 },
  { label: '12 h', seconds: 43200 },
  { label: '24 h', seconds: 86400 },
] as const;

const DEFAULT_DISTANCE_INDEX = 2; // ¼ mi
const DEFAULT_TIME_INDEX = 0; // 30 s

const TIERS: ReadonlyArray<{ key: CoTravelTier; label: string; dot: string }> = [
  { key: 'critical', label: 'CRITICAL', dot: 'bg-red-500' },
  { key: 'high', label: 'HIGH', dot: 'bg-orange-500' },
  { key: 'medium', label: 'MEDIUM', dot: 'bg-yellow-500' },
  { key: 'low', label: 'LOW', dot: 'bg-sky-500' },
  { key: 'minimal', label: 'MINIMAL', dot: 'bg-slate-500' },
];

function miles(m: number): string {
  return `${(m / 1609.34).toFixed(1)} mi`;
}
function minutes(s: number): string {
  return `${(s / 60).toFixed(1)} min`;
}

export default function CoTravel() {
  const [distanceIndex, setDistanceIndex] = useState(DEFAULT_DISTANCE_INDEX);
  const [timeIndex, setTimeIndex] = useState(DEFAULT_TIME_INDEX);

  const minDistanceM = DISTANCE_STOPS[distanceIndex].meters;
  const minTimeS = TIME_STOPS[timeIndex].seconds;

  // Debounce so dragging a slider doesn't fire a request per tick.
  const debouncedDistance = useDebouncedValue(minDistanceM, 300);
  const debouncedTime = useDebouncedValue(minTimeS, 300);

  const params = { min_distance_m: debouncedDistance, min_time_s: debouncedTime, limit: 500 };
  const query = useQuery({
    queryKey: [...queryKeys.coTravel, params],
    queryFn: () => listCoTravel(params),
  });

  const qc = useQueryClient();
  const ignore = useMutation({
    mutationFn: (emitterId: string) => ignoreEmitter(emitterId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.coTravel });
      void qc.invalidateQueries({ queryKey: queryKeys.coTravelIgnored });
    },
  });

  const grouped = useMemo(() => {
    const items = query.data?.items ?? [];
    const byTier = new Map<CoTravelTier, CoTravelItem[]>();
    for (const t of TIERS) byTier.set(t.key, []);
    for (const it of items) byTier.get(it.tier)?.push(it);
    return byTier;
  }, [query.data]);

  const total = query.data?.total ?? 0;
  const fetchedCount = query.data?.items.length ?? 0;
  const isCapped = total > fetchedCount;

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-lg font-semibold text-slate-100">Co-Travel Detection</h1>
        <p className="text-sm text-slate-400">
          Emitters seen at different places and times, ranked by how strongly they behave like they
          followed you.
        </p>
      </div>

      <div className="grid max-w-xl grid-cols-1 gap-4 rounded border border-slate-800 bg-slate-900/40 p-4 sm:grid-cols-2">
        <RangeSlider
          label="Min distance apart"
          stops={DISTANCE_STOPS}
          value={distanceIndex}
          onChange={setDistanceIndex}
        />
        <RangeSlider
          label="Min time apart"
          stops={TIME_STOPS}
          value={timeIndex}
          onChange={setTimeIndex}
        />
      </div>

      <div className="text-sm text-slate-400">
        {query.isLoading
          ? 'Analyzing…'
          : isCapped
            ? `showing top ${fetchedCount} of ${total} emitters`
            : `${total} emitter${total === 1 ? '' : 's'}`}
      </div>

      {query.isError && (
        <div className="rounded border border-red-800 bg-red-950/40 p-3 text-sm text-red-300">
          Failed to load co-travel results.
        </div>
      )}

      <div className="space-y-4">
        {TIERS.map((tier) => {
          const rows = grouped.get(tier.key) ?? [];
          if (rows.length === 0) return null;
          return (
            <section key={tier.key} className="rounded border border-slate-800">
              <header className="flex items-center gap-2 border-b border-slate-800 bg-slate-900/60 px-4 py-2 text-sm font-semibold text-slate-200">
                <span className={`inline-block h-2.5 w-2.5 rounded-full ${tier.dot}`} />
                {tier.label} ({rows.length})
              </header>
              <ul className="divide-y divide-slate-800">
                {rows.map((it) => (
                  <li key={it.emitter_id} className="flex items-center justify-between gap-4 px-4 py-3">
                    <div className="min-w-0">
                      <div className="truncate text-sm text-slate-100">
                        {it.emitter_type ?? 'emitter'} · <span>{it.identity_key ?? it.name}</span>
                      </div>
                      <div className="text-xs text-slate-400">
                        {miles(it.spread_m)} spread · {it.points} points · {minutes(it.span_s)} ·{' '}
                        {it.hits} hits · score {it.score}
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => ignore.mutate(it.emitter_id)}
                      disabled={ignore.isPending}
                      className="shrink-0 rounded border border-slate-700 px-3 py-1 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100 disabled:opacity-50"
                    >
                      Ignore
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
      </div>
    </div>
  );
}

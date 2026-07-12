// Co-Travel Detection: ranks emitters by how strongly they behave like they
// followed you (seen at different places + times), grouped into tiers, with a
// per-row Ignore/Details (map + sparkline). Two snap sliders drive both the
// gate and the score resolution server-side (see design doc §4). An optional
// from/to date window (converted from local `datetime-local` inputs to RFC3339
// UTC) narrows both the ranking query and each row's expanded detections.
import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import RangeSlider from '../components/RangeSlider';
import CoTravelRow from '../components/CoTravelRow';
import IgnoredDrawer from '../components/IgnoredDrawer';
import { queryKeys } from '../api/queryKeys';
import {
  ignoreEmitter,
  listCoTravel,
  listIgnored,
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

export default function CoTravel() {
  const [distanceIndex, setDistanceIndex] = useState(DEFAULT_DISTANCE_INDEX);
  const [timeIndex, setTimeIndex] = useState(DEFAULT_TIME_INDEX);
  const [fromLocal, setFromLocal] = useState('');
  const [toLocal, setToLocal] = useState('');
  const [drawerOpen, setDrawerOpen] = useState(false);

  const minDistanceM = DISTANCE_STOPS[distanceIndex].meters;
  const minTimeS = TIME_STOPS[timeIndex].seconds;

  // Debounce so dragging a slider doesn't fire a request per tick.
  const debouncedDistance = useDebouncedValue(minDistanceM, 300);
  const debouncedTime = useDebouncedValue(minTimeS, 300);

  // datetime-local (local time, no zone) -> RFC3339 UTC; empty -> undefined.
  const fromRfc = fromLocal ? new Date(fromLocal).toISOString() : undefined;
  const toRfc = toLocal ? new Date(toLocal).toISOString() : undefined;

  const params = {
    min_distance_m: debouncedDistance,
    min_time_s: debouncedTime,
    from: fromRfc,
    to: toRfc,
    limit: 500,
  };
  const query = useQuery({
    queryKey: [...queryKeys.coTravel, params],
    queryFn: () => listCoTravel(params),
  });

  const ignoredCountQuery = useQuery({
    queryKey: queryKeys.coTravelIgnored,
    queryFn: listIgnored,
  });
  const ignoredCount = ignoredCountQuery.data?.length ?? 0;

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
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-lg font-semibold text-slate-100">Co-Travel Detection</h1>
          <p className="text-sm text-slate-400">
            Emitters seen at different places and times, ranked by how strongly they behave like they
            followed you.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setDrawerOpen(true)}
          className="shrink-0 rounded border border-slate-700 px-3 py-1.5 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
        >
          Ignored ({ignoredCount})
        </button>
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

      <div className="flex max-w-xl flex-wrap items-end gap-3 rounded border border-slate-800 bg-slate-900/40 p-4">
        <label className="flex flex-col gap-1 text-xs text-slate-400">
          From
          <input
            type="datetime-local"
            aria-label="From"
            value={fromLocal}
            onChange={(e) => setFromLocal(e.target.value)}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-200"
          />
        </label>
        <label className="flex flex-col gap-1 text-xs text-slate-400">
          To
          <input
            type="datetime-local"
            aria-label="To"
            value={toLocal}
            onChange={(e) => setToLocal(e.target.value)}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-200"
          />
        </label>
        {(fromLocal || toLocal) && (
          <button
            type="button"
            onClick={() => {
              setFromLocal('');
              setToLocal('');
            }}
            className="rounded border border-slate-700 px-3 py-1.5 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Clear
          </button>
        )}
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
                  <CoTravelRow
                    key={it.emitter_id}
                    item={it}
                    from={fromRfc}
                    to={toRfc}
                    onIgnore={ignore.mutate}
                    ignoring={ignore.isPending}
                  />
                ))}
              </ul>
            </section>
          );
        })}
      </div>

      <IgnoredDrawer open={drawerOpen} onClose={() => setDrawerOpen(false)} />
    </div>
  );
}

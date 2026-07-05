// Task 9.10: the landing page after login. Composes pieces every earlier
// page already built — no new backend endpoints, no new query keys beyond
// the existing registry (`src/api/queryKeys.ts`).
//
// - KPI row: four `StatTile`s — active data sources (`status === 'running'`
//   out of `listDataSources`), emitter/entity counts, and unread
//   notifications (the authoritative `unread_count` from
//   `GET /api/notifications`, same field `Notifications.tsx` reconciles the
//   header badge to — *not* the live-only `notificationStore` counter, which
//   starts at 0 on page load and would under-report on a fresh session).
// - Live emission feed: the most recent `FEED_LIMIT` emissions, queried
//   under `queryKeys.emissions` (with this page's own suffix appended) so
//   `useLiveEvents` (Task 9.1) invalidating that key on every WS `emission`
//   frame refetches it automatically — same convention as `Emissions.tsx`.
// - Compact map: the existing `pages/MapView` component (Task 9.7),
//   rendered as-is inside a fixed-height container rather than duplicated
//   into a bespoke "compact" variant (YAGNI — it already does exactly what
//   this brief asks for: a heatmap of recent located emissions).
//
// jsdom/test guard: embedding `MapView` pulls in `maplibre-gl`, which needs
// a real WebGL canvas jsdom doesn't have. `Dashboard.test.tsx` mocks
// `maplibre-gl` wholesale, same as `MapView.test.tsx` does.
import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import type { Emission } from '../api/emissions';
import { listEmissions } from '../api/emissions';
import { listDataSources } from '../api/dataSources';
import { listEmitters } from '../api/emitters';
import { listEntities } from '../api/entities';
import { listNotifications } from '../api/notifications';
import StatTile from '../components/StatTile';
import MapView from './MapView';

/** Page size for the feed query — a landing-page glance, not a full browse
 * (that's `Emissions.tsx`'s job), so a modest cap keeps the request light. */
const FEED_LIMIT = 20;

function formatObservedAt(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** Reads a payload key defensively — `payload`'s shape depends on `kind`
 * (see `Emission`'s doc comment), so any of these may be absent. Mirrors
 * `Emissions.tsx`'s helper of the same name/behavior. */
function payloadText(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === 'string' || typeof value === 'number' ? String(value) : '—';
}

export default function Dashboard() {
  const dataSourcesQuery = useQuery({ queryKey: queryKeys.dataSources, queryFn: listDataSources });
  const emittersQuery = useQuery({ queryKey: queryKeys.emitters, queryFn: listEmitters });
  const entitiesQuery = useQuery({ queryKey: queryKeys.entities, queryFn: listEntities });

  // Small, dedicated fetch for the authoritative unread count — separate
  // cache entry from `Notifications.tsx`'s own paginated/filtered query, but
  // both share the `queryKeys.notifications` prefix so a WS `notification`
  // frame's `invalidateQueries({ queryKey: queryKeys.notifications })`
  // refreshes this tile too.
  const notificationsSummaryQuery = useQuery({
    queryKey: [...queryKeys.notifications, 'dashboard-summary'],
    queryFn: () => listNotifications({ limit: 1 }),
  });

  const feedParams = useMemo(() => {
    const params = new URLSearchParams();
    params.set('limit', String(FEED_LIMIT));
    return params;
  }, []);

  const feedQuery = useQuery({
    queryKey: [...queryKeys.emissions, 'dashboard-feed', feedParams.toString()],
    queryFn: () => listEmissions(feedParams),
  });

  const activeDataSourceCount = (dataSourcesQuery.data ?? []).filter(
    (source) => source.status === 'running',
  ).length;

  const emitterNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const emitter of emittersQuery.data ?? []) map.set(emitter.id, emitter.name);
    return map;
  }, [emittersQuery.data]);

  function emitterNameFor(emission: Emission): string {
    return emission.emitter_id ? (emitterNameById.get(emission.emitter_id) ?? '—') : '—';
  }

  const feedItems = feedQuery.data?.items ?? [];

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold text-slate-100">Dashboard</h1>

      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <StatTile
          label="Active Data Sources"
          value={dataSourcesQuery.data ? activeDataSourceCount : null}
          loading={dataSourcesQuery.isLoading}
        />
        <StatTile
          label="Emitters"
          value={emittersQuery.data?.length}
          loading={emittersQuery.isLoading}
        />
        <StatTile
          label="Entities"
          value={entitiesQuery.data?.length}
          loading={entitiesQuery.isLoading}
        />
        <StatTile
          label="Unread Notifications"
          value={notificationsSummaryQuery.data?.unread_count}
          loading={notificationsSummaryQuery.isLoading}
        />
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <section className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/40 p-4">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-slate-400">Live Emission Feed</h2>

          {feedQuery.isLoading && <p className="text-sm text-slate-500">Loading emissions…</p>}
          {feedQuery.isError && <p className="text-sm text-red-400">Failed to load emissions.</p>}
          {feedQuery.data && feedItems.length === 0 && (
            <p className="text-sm text-slate-500">No emissions yet.</p>
          )}

          {feedItems.length > 0 && (
            <div className="max-h-[420px] overflow-y-auto">
              <table className="w-full border-collapse text-left text-sm">
                <thead>
                  <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
                    <th className="py-1.5 pr-3 font-medium">Observed At</th>
                    <th className="py-1.5 pr-3 font-medium">BSSID</th>
                    <th className="py-1.5 pr-3 font-medium">SSID</th>
                    <th className="py-1.5 pr-3 font-medium">Channel</th>
                    <th className="py-1.5 pr-3 font-medium">RSSI</th>
                    <th className="py-1.5 pr-3 font-medium">Emitter</th>
                  </tr>
                </thead>
                <tbody>
                  {feedItems.map((emission) => (
                    <tr
                      key={emission.id}
                      data-testid={`dashboard-feed-row-${emission.id}`}
                      className="border-b border-slate-900 align-top"
                    >
                      <td className="py-1.5 pr-3 text-slate-300">{formatObservedAt(emission.observed_at)}</td>
                      <td className="py-1.5 pr-3 font-mono text-slate-300">
                        {payloadText(emission.payload, 'bssid')}
                      </td>
                      <td className="py-1.5 pr-3 text-slate-300">{payloadText(emission.payload, 'ssid')}</td>
                      <td className="py-1.5 pr-3 text-slate-300">{payloadText(emission.payload, 'channel')}</td>
                      <td className="py-1.5 pr-3 font-mono text-slate-300">{emission.signal_strength ?? '—'}</td>
                      <td className="py-1.5 pr-3 text-slate-300">{emitterNameFor(emission)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="h-[560px] overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40 p-2">
          <MapView />
        </section>
      </div>
    </div>
  );
}

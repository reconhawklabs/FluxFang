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
// - GPS Status block (Phase 5): `GET /api/gps/status`, polled on a short
//   interval so the state/lat/lon read as "live" without needing a WS frame
//   of their own (see `queryKeys.ts`'s note on why `gpsStatus` isn't
//   WS-invalidated).
//
// jsdom/test guard: embedding `MapView` pulls in `maplibre-gl`, which needs
// a real WebGL canvas jsdom doesn't have. `Dashboard.test.tsx` mocks
// `maplibre-gl` wholesale, same as `MapView.test.tsx` does.
import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { queryKeys } from "../api/queryKeys";
import type { Emission } from "../api/emissions";
import { listEmissions } from "../api/emissions";
import { isEmittingSource, listDataSources } from "../api/dataSources";
import { listEmitters } from "../api/emitters";
import { listEntities } from "../api/entities";
import { listNotifications } from "../api/notifications";
import type { GpsSourceStatus, GpsStatus } from "../api/gps";
import { getGpsStatus } from "../api/gps";
import StatTile from "../components/StatTile";
import MapView from "./MapView";

/** Page size for the feed query — a landing-page glance, not a full browse
 * (that's `Emissions.tsx`'s job), so a modest cap keeps the request light. */
const FEED_LIMIT = 20;

/** How often the GPS Status block re-polls `GET /api/gps/status` — short
 * enough to read as "live" (a fix's age/quality can change every second)
 * without hammering the endpoint. */
const GPS_STATUS_REFETCH_MS = 4000;

/** The dashboard's time-range selector — a sliding "past N" window applied to
 * both the live feed and the embedded map, replacing the map's own From/To
 * pickers with an at-a-glance choice. */
const TIME_RANGES = [
  { id: "15m", label: "Past 15 min", ms: 15 * 60 * 1000 },
  { id: "1h", label: "Past hour", ms: 60 * 60 * 1000 },
  { id: "4h", label: "Past 4 hours", ms: 4 * 60 * 60 * 1000 },
  { id: "24h", label: "Past 24 hours", ms: 24 * 60 * 60 * 1000 },
] as const;
type TimeRangeId = (typeof TIME_RANGES)[number]["id"];
const DEFAULT_RANGE_ID: TimeRangeId = "1h";

/** How often the sliding window's `from` bound is recomputed. Coarse on
 * purpose: the window's lower bound moves in 30s steps rather than every
 * render, so the feed/map query keys stay stable (no refetch storm) while
 * still keeping "past N" honest. */
const TIME_WINDOW_REFRESH_MS = 30_000;

const GPS_STATUS_LABEL: Record<GpsSourceStatus, string> = {
  disabled: "GPS disabled / no source",
  acquiring: "Acquiring signal…",
  active: "Active",
  degraded: "Degraded signal",
};

/** Status dot + text color — green for a good fix, amber while
 * acquiring/degraded (something's happening but not yet trustworthy), gray
 * when there's no gps source running at all. */
const GPS_STATUS_COLOR: Record<GpsSourceStatus, string> = {
  disabled: "bg-slate-500 text-slate-400",
  acquiring: "bg-amber-400 text-amber-400",
  active: "bg-emerald-400 text-emerald-400",
  degraded: "bg-orange-400 text-orange-400",
};

function formatCoord(value: number): string {
  return value.toFixed(5);
}

function GpsStatusBlock() {
  const gpsQuery = useQuery({
    queryKey: queryKeys.gpsStatus,
    queryFn: getGpsStatus,
    refetchInterval: GPS_STATUS_REFETCH_MS,
  });

  const gps: GpsStatus | undefined = gpsQuery.data;
  const [dotClass, textClass] = (
    gps ? GPS_STATUS_COLOR[gps.status] : "bg-slate-700 text-slate-500"
  ).split(" ");
  const hasCoords = Boolean(
    gps?.has_fix && gps.lat !== null && gps.lon !== null,
  );

  return (
    <section
      data-testid="gps-status-block"
      className="max-w-[220px] rounded-lg border border-slate-700/80 bg-slate-900/85 px-3 py-2 shadow-lg backdrop-blur-sm"
    >
      <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        GPS Status
      </h2>
      <div className="mt-2 flex items-center gap-2">
        <span
          className={`h-2.5 w-2.5 shrink-0 rounded-full ${dotClass}`}
          aria-hidden="true"
        />
        <span className={`text-sm font-medium ${textClass}`}>
          {gps ? GPS_STATUS_LABEL[gps.status] : "—"}
        </span>
      </div>
      <p className="mt-2 font-mono text-sm text-slate-300">
        {hasCoords && gps
          ? `${formatCoord(gps.lat as number)}, ${formatCoord(gps.lon as number)}`
          : "—"}
      </p>
      {hasCoords && gps && gps.fix_age_seconds !== null && (
        <p className="mt-1 text-xs text-slate-500">
          Fix age: {Math.round(gps.fix_age_seconds)}s
        </p>
      )}
    </section>
  );
}

/** Feed source-tab styling — an amber underline marks the active tab. */
function feedTabClassName(active: boolean): string {
  return `-mb-px border-b-2 px-3 py-1.5 text-sm font-medium ${
    active
      ? "border-amber-500 text-amber-400"
      : "border-transparent text-slate-400 hover:text-slate-200"
  }`;
}

function formatObservedAt(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** Reads a payload key defensively — `payload`'s shape depends on `kind`
 * (see `Emission`'s doc comment), so any of these may be absent. Mirrors
 * `Emissions.tsx`'s helper of the same name/behavior. */
function payloadText(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : "—";
}

export default function Dashboard() {
  const [rangeId, setRangeId] = useState<TimeRangeId>(DEFAULT_RANGE_ID);
  // `feedSourceId === null` is the default "All Emissions" tab; otherwise it's
  // a specific data source's id (a per-source feed tab).
  const [feedSourceId, setFeedSourceId] = useState<string | null>(null);

  // Advances every `TIME_WINDOW_REFRESH_MS` so the sliding window's lower
  // bound stays current without recomputing on every render.
  const [nowTick, setNowTick] = useState(() => Date.now());
  useEffect(() => {
    const interval = setInterval(
      () => setNowTick(Date.now()),
      TIME_WINDOW_REFRESH_MS,
    );
    return () => clearInterval(interval);
  }, []);

  const rangeMs =
    TIME_RANGES.find((range) => range.id === rangeId)?.ms ?? TIME_RANGES[1].ms;
  const timeFrom = useMemo(
    () => new Date(nowTick - rangeMs).toISOString(),
    [nowTick, rangeMs],
  );

  const dataSourcesQuery = useQuery({
    queryKey: queryKeys.dataSources,
    queryFn: listDataSources,
  });
  // Interim `{limit: 500}` cap on the *items* side — `GET /api/emitters`
  // now returns a paginated `{items, total}` envelope. The KPI tile below
  // uses the authoritative `.total` (cheaper than counting an array and
  // correct even past the cap); `emitterNameById` still needs the actual
  // rows to resolve feed emitter names, hence the cap here rather than
  // dropping the fetch.
  const emittersQuery = useQuery({
    queryKey: queryKeys.emitters,
    queryFn: () => listEmitters({ limit: 500 }),
  });
  const entitiesQuery = useQuery({
    queryKey: queryKeys.entities,
    queryFn: () => listEntities({ limit: 500 }),
  });

  // Small, dedicated fetch for the authoritative unread count — separate
  // cache entry from `Notifications.tsx`'s own paginated/filtered query, but
  // both share the `queryKeys.notifications` prefix so a WS `notification`
  // frame's `invalidateQueries({ queryKey: queryKeys.notifications })`
  // refreshes this tile too.
  const notificationsSummaryQuery = useQuery({
    queryKey: [...queryKeys.notifications, "dashboard-summary"],
    queryFn: () => listNotifications({ limit: 1 }),
  });

  const feedParams = useMemo(() => {
    const params = new URLSearchParams();
    params.set("limit", String(FEED_LIMIT));
    params.set("time_from", timeFrom);
    if (feedSourceId) params.set("data_source_id", feedSourceId);
    return params;
  }, [timeFrom, feedSourceId]);

  const feedQuery = useQuery({
    queryKey: [...queryKeys.emissions, "dashboard-feed", feedParams.toString()],
    queryFn: () => listEmissions(feedParams),
  });

  const activeDataSourceCount = (dataSourcesQuery.data ?? []).filter(
    (source) => source.status === "running",
  ).length;

  const emitterNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const emitter of emittersQuery.data?.items ?? [])
      map.set(emitter.id, emitter.name);
    return map;
  }, [emittersQuery.data]);

  function emitterNameFor(emission: Emission): string {
    return emission.emitter_id
      ? (emitterNameById.get(emission.emitter_id) ?? "—")
      : "—";
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
          value={emittersQuery.data?.total}
          loading={emittersQuery.isLoading}
        />
        <StatTile
          label="Entities"
          value={entitiesQuery.data?.total}
          loading={entitiesQuery.isLoading}
        />
        <StatTile
          label="Unread Notifications"
          value={notificationsSummaryQuery.data?.unread_count}
          loading={notificationsSummaryQuery.isLoading}
        />
      </div>

      {/* Time Range + GPS status now live as overlays inside the map (see the
          MapView props below), so there's no separate card row — the map gets
          the full width the two cards used to take. Feed narrows to a third,
          map takes two thirds. */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <section className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/40 p-4 lg:col-span-1">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-slate-400">
            Live Emission Feed
          </h2>

          {/* Per-source tabs; "All Emissions" (default) is first. */}
          <div className="flex flex-wrap gap-1 border-b border-slate-800">
            <button
              type="button"
              onClick={() => setFeedSourceId(null)}
              className={feedTabClassName(feedSourceId === null)}
            >
              All Emissions
            </button>
            {(dataSourcesQuery.data ?? [])
              .filter(isEmittingSource)
              .map((source) => (
                <button
                  key={source.id}
                  type="button"
                  onClick={() => setFeedSourceId(source.id)}
                  className={feedTabClassName(feedSourceId === source.id)}
                >
                  {source.kind} ({source.interface ?? source.id})
                </button>
              ))}
          </div>

          {feedQuery.isLoading && (
            <p className="text-sm text-slate-500">Loading emissions…</p>
          )}
          {feedQuery.isError && (
            <p className="text-sm text-red-400">Failed to load emissions.</p>
          )}
          {feedQuery.data && feedItems.length === 0 && (
            <p className="text-sm text-slate-500">No emissions yet.</p>
          )}

          {feedItems.length > 0 && (
            <div className="max-h-[520px] overflow-y-auto">
              <table className="w-full border-collapse text-left text-sm">
                <thead>
                  <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
                    <th className="py-1.5 pr-3 font-medium">Observed At</th>
                    <th className="py-1.5 pr-3 font-medium">SSID</th>
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
                      <td className="py-1.5 pr-3 text-slate-300">
                        {formatObservedAt(emission.observed_at)}
                      </td>
                      <td className="py-1.5 pr-3 text-slate-300">
                        {payloadText(emission.payload, "ssid")}
                      </td>
                      <td className="py-1.5 pr-3 font-mono text-slate-300">
                        {emission.signal_strength ?? "—"}
                      </td>
                      <td className="py-1.5 pr-3 text-slate-300">
                        {emitterNameFor(emission)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="h-[640px] overflow-hidden rounded-lg border border-slate-800 bg-slate-900/40 p-2 lg:col-span-2">
          <MapView
            showControls={false}
            basemap="satellite"
            timeFrom={timeFrom}
            timeTo=""
            overlayTopLeft={
              <div className="rounded-lg border border-slate-700/80 bg-slate-900/85 px-3 py-2 shadow-lg backdrop-blur-sm">
                <label
                  htmlFor="dashboard-range"
                  className="text-[10px] font-semibold uppercase tracking-wide text-slate-500"
                >
                  Time Range
                </label>
                <select
                  id="dashboard-range"
                  value={rangeId}
                  onChange={(event) =>
                    setRangeId(event.target.value as TimeRangeId)
                  }
                  className="mt-1 block w-full rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
                >
                  {TIME_RANGES.map((range) => (
                    <option key={range.id} value={range.id}>
                      {range.label}
                    </option>
                  ))}
                </select>
              </div>
            }
            overlayBottomLeft={<GpsStatusBlock />}
          />
        </section>
      </div>
    </div>
  );
}

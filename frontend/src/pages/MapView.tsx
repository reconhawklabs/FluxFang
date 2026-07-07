// Task 9.7: Map page — a MapLibre GL map showing an emissions heatmap,
// entity markers, and a zones overlay, with layer toggles + a scoping
// filter row.
//
// Phase 6 (Map page redesign addendum) restructured the control panel and
// added a basemap switcher. A later refinement removed the "Emissions"
// category-toggle group (and its per-category heatmap layers) as redundant
// with the "Sources" group, which is now the sole emission filter; a single
// aggregate heatmap is always shown, scoped by Sources.
//
// This component is shared: the `/map` route renders it with its full control
// panel, and `Dashboard.tsx` embeds it with `showControls={false}` +
// `basemap="satellite"` + a caller-driven `timeFrom`/`timeTo` window (see the
// `MapViewProps` doc comments).
//
// Style: a hand-built keyless *raster* style — not a vector style from a
// hosted (API-keyed) provider like MapTiler/Mapbox — this app has no
// map-tile API key configured anywhere. NOTE: every basemap option (including
// "Standard") still needs *runtime internet access* for the browser to fetch
// tile images; the data layers below (heatmap/entities/emitters/zones, built
// from this app's own API) render regardless of whether tiles load, they
// just draw on a blank background if the tile host is unreachable.
//
// Data sources for the layers:
//   - Emissions heatmap: `GET /api/emissions/points` (Task 8 — scoped by the
//     Sources group + the time window, same as the old `GET /api/emissions`
//     query it replaced), shaped by `emissionPointsToHeatmapGeoJSON`
//     (`components/mapData.ts`). Unlike the old page-sized `GET /api/emissions`
//     list (capped at 500, oldest points silently dropped on a long drive),
//     the points endpoint is uncapped up to its own server-side safety cap
//     (`MAX_POINTS`, 50,000) and reports `truncated` when that cap is hit —
//     see `anyTruncated` below.
//   - Emitter markers still read from a separate `GET /api/emissions` query
//     (`allEmissionsQueries`/`emissionsItems` below) — the points endpoint
//     returns bare coordinates with no `emitter_id` to group by.
//   - Entity markers: `GET /api/entities` for the id/name list, then
//     `GET /api/entities/:id` per entity (parallel, via `useQueries`) for
//     `recent_detections`; each entity's marker sits at its detection with
//     the latest `observed_at` (i.e. "most recent known location"), not a
//     live/current position. An entity with no detections yet gets no
//     marker. This is one request per entity — fine at this app's expected
//     entity counts (a handful to a few dozen known devices/people), and
//     documented here as the YAGNI-appropriate approach rather than adding a
//     bulk "last location per entity" backend endpoint this task doesn't
//     otherwise need.
//   - Emitter markers (Phase 6, "Layers" group's Emitters toggle): one
//     marker per emitter, derived client-side from the SAME "all emissions"
//     query's items via `emitterMarkersFromEmissions` (groups located
//     emissions by `emitter_id`, keeps only the latest per emitter) — no
//     separate backend request, since the all-emissions query already fetches
//     the located emissions this needs.
//   - Zones: `GET /api/zones`, shaped by `zonesToCircleGeoJSON` into a
//     circle-approximation polygon per zone.
//
// Controls (Map page only; hidden when `showControls={false}`), left→right:
//   - Basemap switcher (Standard/Satellite/Dark): see `components/
//     basemapStyles.ts`. Hidden when the `basemap` prop pins it.
//   - From/To datetime pickers (hidden when the caller drives the window via
//     the `timeFrom`/`timeTo` props).
//   - "Layers" checkbox group: Zones / Entities / Emitters, independent of
//     each other.
//   - "Sources" checkbox group: "All Sources" (master, default) + one
//     checkbox per `GET /api/data-sources` entry, replacing the old
//     single-select dropdown. Unlike the Emissions/Layers groups (which only
//     toggle a layer's *visibility*), a Sources checkbox changes what's
//     *fetched*: with "All Sources" unchecked, each checked source becomes
//     its own `GET /api/emissions?data_source_id=<id>` query (see
//     `sourceScopeIds` below) and the results are unioned client-side — the
//     backend only takes one `data_source_id` per request, so a multi-source
//     selection means "query per selected source, merge the items." With zero
//     sources checked (and "All Sources" unchecked), nothing is fetched —
//     the user has explicitly filtered everything out.
//
// jsdom/test guard: MapLibre GL needs a real WebGL canvas, which jsdom
// doesn't provide. Rather than special-casing the component for tests, the
// test file (`MapView.test.tsx`) `vi.mock('maplibre-gl')`s the whole module
// so `new maplibregl.Map(...)` never touches a real canvas; this component
// itself just does normal effect-based init and trusts that mock in tests.
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import maplibregl from "maplibre-gl";
import type {
  GeoJSONSource,
  Map as MapLibreMap,
  MapLayerMouseEvent,
  Popup,
  RasterTileSource,
  StyleSpecification,
} from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { queryKeys } from "../api/queryKeys";
import { listEmissionPoints, listEmissions } from "../api/emissions";
import { getEntityDetail, listEntities } from "../api/entities";
import { listZones } from "../api/zones";
import { isEmittingSource, listDataSources } from "../api/dataSources";
import { listEmitters } from "../api/emitters";
import type { Emitter } from "../api/emitters";
import { getGpsStatus } from "../api/gps";
import type { EntityMarker } from "../components/mapData";
import {
  emissionPointsToHeatmapGeoJSON,
  emitterMarkersFromEmissions,
  entitiesToMarkerFeatures,
  zonesToCircleGeoJSON,
} from "../components/mapData";
import {
  BASEMAP_OPTIONS,
  DEFAULT_BASEMAP_ID,
  basemapOption,
} from "../components/basemapStyles";
import type { BasemapId } from "../components/basemapStyles";

/** How often the GPS fix backing center-on-load/recenter is re-polled —
 * same cadence as the Dashboard's GPS Status block (`Dashboard.tsx`), so
 * "recenter to me" uses a reasonably fresh fix without an explicit refetch
 * on click. */
const GPS_STATUS_REFETCH_MS = 4000;

/** Zoom level used when centering on the user's location — close enough to
 * be useful (street-level-ish) without requiring the fix to be pixel-exact. */
const USER_LOCATION_ZOOM = 14;

const BASEMAP_SOURCE_ID = "basemap-source";
const BASEMAP_LAYER_ID = "basemap-tiles";

const EMISSIONS_SOURCE_ID = "emissions-heatmap-source";
const ENTITIES_SOURCE_ID = "entity-markers-source";
const EMITTERS_SOURCE_ID = "emitter-markers-source";
const ZONES_SOURCE_ID = "zones-source";

const HEATMAP_LAYER_IDS = ["emissions-heatmap-layer"];
const ENTITY_LAYER_IDS = ["entity-circle-layer", "entity-label-layer"];
const EMITTER_LAYER_IDS = ["emitter-circle-layer", "emitter-label-layer"];
const ZONE_LAYER_IDS = [
  "zone-fill-layer",
  "zone-line-layer",
  "zone-label-layer",
];

const EMPTY_POINT_FC = { type: "FeatureCollection" as const, features: [] };
const EMPTY_POLYGON_FC = { type: "FeatureCollection" as const, features: [] };

// Task 8: the heatmap itself no longer uses a client-side limit — it's fed by
// `GET /api/emissions/points`, which is uncapped up to its own server-side
// `MAX_POINTS` safety cap (see the module doc comment). This cap now bounds
// only the SEPARATE `allEmissionsQueries` fetch below, which exists solely to
// resolve emitter markers (the points endpoint has no `emitter_id` to group
// by) — same "interim cap, fine at today's scale" rationale as the
// `emittersQuery`/`entitiesQuery` limits further down.
const EMITTER_MARKER_EMISSIONS_LIMIT = 500;

/** Fallback map view when there's no GPS fix to center on: roughly the
 * geographic center of the contiguous United States, zoomed out to show
 * North America. Beats the old `[0, 0]`/zoom-2 (open ocean off West Africa),
 * which read as a blank/empty map to a user without a GPS dongle. Overridden
 * by the center-on-user-load effect the moment a fix is known. */
const DEFAULT_CENTER: [number, number] = [-98.5, 39.5];
const DEFAULT_ZOOM = 3.5;

interface LayerVisibility {
  zones: boolean;
  entities: boolean;
  emitters: boolean;
}

const DEFAULT_LAYER_VISIBILITY: LayerVisibility = {
  zones: true,
  entities: true,
  emitters: true,
};

/** Shared `GET /api/emissions`(`/points`) params — time-range scoping plus an
 * optional `data_source_id` (Sources group, see module doc comment). No
 * `limit`: the points endpoint ignores any client-supplied limit/offset
 * (server-side `MAX_POINTS` cap instead, see module doc comment), and the
 * separate emitter-markers `allEmissionsQueries` fetch below sets its own
 * `limit` directly on top of this (see that call site). `timeFrom`/`timeTo`
 * are already RFC3339 strings when they come from the Dashboard's range
 * selector, or `datetime-local` strings from the Map page's pickers; both
 * parse cleanly through `new Date(...)`. */
function buildEmissionsParams(opts: {
  dataSourceId: string;
  timeFrom: string;
  timeTo: string;
}): URLSearchParams {
  const params = new URLSearchParams();
  if (opts.dataSourceId.length > 0)
    params.set("data_source_id", opts.dataSourceId);
  if (opts.timeFrom.length > 0)
    params.set("time_from", new Date(opts.timeFrom).toISOString());
  if (opts.timeTo.length > 0)
    params.set("time_to", new Date(opts.timeTo).toISOString());
  return params;
}

/** HTML-escape a value bound for a MapLibre popup's `setHTML` (emitter
 * names/SSIDs/MACs are user-influenced, so treat them as untrusted). */
function escapeHtml(value: string): string {
  return value.replace(
    /[&<>"']/g,
    (char) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[char] as string,
  );
}

function formatDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** Inner HTML for an emitter marker's click popup. Uses the rich fields
 * (type, identifying MAC/SSID, last-seen) from the full `Emitter` when it's
 * been matched by id from `emittersQuery`; falls back to just the marker's own
 * name/observed-at otherwise. Styled inline (dark theme) on top of the
 * `.fluxfang-map-popup` container override in `index.css`. */
function buildEmitterPopupHtml(
  emitter: Emitter | undefined,
  fallbackName: string,
  observedAt: string | null,
): string {
  const attrs = emitter?.attributes ?? {};
  const str = (key: string): string | null =>
    typeof attrs[key] === "string" ? (attrs[key] as string) : null;
  const mac = str("bssid") ?? str("src_mac");
  const ssid = str("ssid");
  const randomized = attrs.randomized_mac === true;
  const lastSeen = emitter?.last_seen_at ?? observedAt;

  const row = (label: string, valueHtml: string): string =>
    `<div style="margin-top:4px;font-size:0.75rem;line-height:1.25"><span style="color:#64748b">${label}: </span><span style="color:#cbd5e1">${valueHtml}</span></div>`;

  const parts: string[] = [
    `<div style="font-weight:600;font-size:0.875rem;color:#f1f5f9">${escapeHtml(
      emitter?.name ?? fallbackName,
    )}</div>`,
    `<div style="font-size:0.75rem;color:#94a3b8;margin-top:2px">${escapeHtml(
      emitter?.type_label ?? emitter?.type ?? "Emitter",
    )}</div>`,
  ];
  if (ssid) parts.push(row("SSID", escapeHtml(ssid)));
  if (mac) {
    parts.push(
      row(
        "MAC",
        `<span style="font-family:ui-monospace,monospace">${escapeHtml(mac)}</span>${
          randomized ? ' <span style="color:#fbbf24">(randomized)</span>' : ""
        }`,
      ),
    );
  }
  if (lastSeen)
    parts.push(row("Last seen", escapeHtml(formatDateTime(lastSeen))));
  return `<div style="min-width:150px">${parts.join("")}</div>`;
}

/** Phase 6: the map's initial (and only ever) style — a single raster
 * source/layer for whichever basemap is selected. Switching basemaps swaps
 * this source's tiles in place (see `components/basemapStyles.ts`'s module
 * doc comment) rather than replacing the style, so this is only ever called
 * once, at map construction, with `DEFAULT_BASEMAP_ID`. */
function buildBasemapStyle(basemapId: BasemapId): StyleSpecification {
  const option = basemapOption(basemapId);
  return {
    version: 8,
    sources: {
      [BASEMAP_SOURCE_ID]: {
        type: "raster",
        tiles: option.tiles,
        tileSize: option.tileSize,
        attribution: option.attribution,
      },
    },
    layers: [
      { id: BASEMAP_LAYER_ID, type: "raster", source: BASEMAP_SOURCE_ID },
    ],
  };
}

/** Adds every source/layer this page draws, once the style has finished
 * loading. Split out of the init effect so it's obvious this only ever
 * touches a freshly-created map on its `'load'` event. The basemap source
 * itself is already part of the style passed to `new maplibregl.Map(...)`,
 * so it isn't added again here. */
function addLayers(map: MapLibreMap): void {
  map.addSource(EMISSIONS_SOURCE_ID, { type: "geojson", data: EMPTY_POINT_FC });
  map.addLayer({
    id: "emissions-heatmap-layer",
    type: "heatmap",
    source: EMISSIONS_SOURCE_ID,
    paint: {
      "heatmap-weight": 1,
      "heatmap-intensity": 1,
      "heatmap-radius": 20,
      "heatmap-opacity": 0.75,
    },
  });

  map.addSource(ENTITIES_SOURCE_ID, { type: "geojson", data: EMPTY_POINT_FC });
  map.addLayer({
    id: "entity-circle-layer",
    type: "circle",
    source: ENTITIES_SOURCE_ID,
    paint: {
      "circle-radius": 6,
      "circle-color": "#f59e0b",
      "circle-stroke-width": 2,
      "circle-stroke-color": "#0f172a",
    },
  });
  map.addLayer({
    id: "entity-label-layer",
    type: "symbol",
    source: ENTITIES_SOURCE_ID,
    layout: {
      "text-field": ["get", "name"],
      "text-size": 12,
      "text-offset": [0, 1.2],
      "text-anchor": "top",
    },
    paint: {
      "text-color": "#f8fafc",
      "text-halo-color": "#0f172a",
      "text-halo-width": 1,
    },
  });

  // Phase 6's "Emitters" layer — same marker styling approach as entities,
  // a distinct color (cyan) so the two aren't confused on the map.
  map.addSource(EMITTERS_SOURCE_ID, { type: "geojson", data: EMPTY_POINT_FC });
  map.addLayer({
    id: "emitter-circle-layer",
    type: "circle",
    source: EMITTERS_SOURCE_ID,
    paint: {
      "circle-radius": 5,
      "circle-color": "#22d3ee",
      "circle-stroke-width": 2,
      "circle-stroke-color": "#0f172a",
    },
  });
  map.addLayer({
    id: "emitter-label-layer",
    type: "symbol",
    source: EMITTERS_SOURCE_ID,
    layout: {
      "text-field": ["get", "name"],
      "text-size": 11,
      "text-offset": [0, 1.1],
      "text-anchor": "top",
    },
    paint: {
      "text-color": "#a5f3fc",
      "text-halo-color": "#0f172a",
      "text-halo-width": 1,
    },
  });

  map.addSource(ZONES_SOURCE_ID, { type: "geojson", data: EMPTY_POLYGON_FC });
  map.addLayer({
    id: "zone-fill-layer",
    type: "fill",
    source: ZONES_SOURCE_ID,
    paint: { "fill-color": "#38bdf8", "fill-opacity": 0.15 },
  });
  map.addLayer({
    id: "zone-line-layer",
    type: "line",
    source: ZONES_SOURCE_ID,
    paint: { "line-color": "#38bdf8", "line-width": 2 },
  });
  map.addLayer({
    id: "zone-label-layer",
    type: "symbol",
    source: ZONES_SOURCE_ID,
    layout: { "text-field": ["get", "name"], "text-size": 12 },
    paint: {
      "text-color": "#38bdf8",
      "text-halo-color": "#0f172a",
      "text-halo-width": 1,
    },
  });
}

const inputClassName =
  "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none";
const labelClassName =
  "block text-xs font-medium uppercase tracking-wide text-slate-500";
// A "Layers"/"Sources" control group: a bordered box (`groupBoxClassName`)
// with the group name on the border (`groupLegendClassName`) and its
// checkboxes laid out in a horizontal wrapping row (`groupCheckboxRowClassName`).
const groupBoxClassName = "rounded border border-slate-800 px-3 pb-2 pt-1";
const groupLegendClassName =
  "px-1 text-xs font-semibold uppercase tracking-wide text-slate-400";
const groupCheckboxRowClassName = "flex flex-wrap items-center gap-x-4 gap-y-1";
const checkboxInputClassName =
  "h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500 disabled:cursor-not-allowed disabled:opacity-50";
const checkboxLabelClassName =
  "flex items-center gap-2 text-sm text-slate-300 has-[:disabled]:text-slate-500";

interface MapViewProps {
  /** When false, the control panel (From/To, Layers, Sources, basemap) and
   *  the page `<h1>` are hidden — the embedding page (Dashboard) supplies its
   *  own chrome and just wants the map surface. Default true (the `/map`
   *  route). */
  showControls?: boolean;
  /** Pins the basemap and hides the in-map switcher (Dashboard pins this to
   *  `'satellite'` for an at-a-glance view). When omitted, the basemap is
   *  user-switchable, starting at `DEFAULT_BASEMAP_ID`. */
  basemap?: BasemapId;
  /** Time window applied to the emissions heatmap query. When provided
   *  (Dashboard's 5m/1h/24h range selector), it overrides — and hides — the
   *  Map page's own From/To pickers. RFC3339 strings; empty string = no
   *  bound. */
  timeFrom?: string;
  timeTo?: string;
  /** Overlay slots rendered *inside* the map surface (the Dashboard uses these
   *  to sit its Time Range control and GPS status readout in the map's
   *  corners instead of as separate cards above it). `overlayTopLeft` stacks
   *  under the recenter button; `overlayBottomLeft` sits in the bottom-left.
   *  Each should be a compact, self-contained element — MapView only handles
   *  positioning. */
  overlayTopLeft?: ReactNode;
  overlayBottomLeft?: ReactNode;
}

export default function MapView({
  showControls = true,
  basemap,
  timeFrom: timeFromProp,
  timeTo: timeToProp,
  overlayTopLeft,
  overlayBottomLeft,
}: MapViewProps = {}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const [styleLoaded, setStyleLoaded] = useState(false);
  const [autoTrack, setAutoTrack] = useState(false);

  // "Layers" group: independent of each other.
  const [layerVisibility, setLayerVisibility] = useState<LayerVisibility>(
    DEFAULT_LAYER_VISIBILITY,
  );

  // "Sources" group (replaces the old single-select data-source dropdown):
  // "All Sources" is the master toggle, default checked (no filter);
  // `sourceSelected` only matters once it's unchecked, and — unlike the
  // Emissions/Layers groups — changes what's *fetched*, not just what's
  // *shown* (see `sourceScopeIds` below).
  const [allSources, setAllSources] = useState(true);
  const [sourceSelected, setSourceSelected] = useState<Record<string, boolean>>(
    {},
  );

  // From/To datetime pickers — native `<input type="datetime-local">`,
  // converted to RFC3339 `time_from`/`time_to` params by
  // `buildEmissionsParams`. Clearing either removes that bound. Overridden by
  // the `timeFrom`/`timeTo` props when the Dashboard drives the window.
  const [timeFromState, setTimeFrom] = useState("");
  const [timeToState, setTimeTo] = useState("");
  const timeFrom = timeFromProp ?? timeFromState;
  const timeTo = timeToProp ?? timeToState;

  // Basemap switcher (Standard/Satellite/Dark) — see `components/
  // basemapStyles.ts`'s module doc comment for the swap-tiles-in-place
  // approach. The `basemap` prop, when set, pins it (no switcher).
  const [basemapState, setBasemapId] = useState<BasemapId>(
    basemap ?? DEFAULT_BASEMAP_ID,
  );
  const basemapId = basemap ?? basemapState;

  const dataSourcesQuery = useQuery({
    queryKey: queryKeys.dataSources,
    queryFn: listDataSources,
  });

  // Which `data_source_id` values to query for (Sources group): `['']`
  // (an empty string means "no data_source_id param," i.e. unfiltered) when
  // "All Sources" is checked; otherwise the checked sources' ids — `[]` if
  // none are checked, meaning nothing is fetched at all.
  const sourceScopeIds = useMemo<string[]>(() => {
    if (allSources) return [""];
    return (dataSourcesQuery.data ?? [])
      .map((source) => source.id)
      .filter((id) => sourceSelected[id] === true);
  }, [allSources, dataSourcesQuery.data, sourceSelected]);

  // Emitter markers' source data: one query per selected source (see
  // `sourceScopeIds` above), unioned client-side — the backend only accepts
  // a single `data_source_id` per request. NOT the heatmap's data source
  // (see `allPointsQueries` below) — this fetch exists only to resolve
  // `emitter_id`s (via `emitterMarkersFromEmissions`), which the points
  // endpoint doesn't carry. `EMITTER_MARKER_EMISSIONS_LIMIT` is set directly
  // on top of `buildEmissionsParams`'s output since that helper itself no
  // longer takes a `limit` (see its doc comment).
  const allEmissionsQueries = useQueries({
    queries: sourceScopeIds.map((sourceId) => {
      const params = buildEmissionsParams({
        dataSourceId: sourceId,
        timeFrom,
        timeTo,
      });
      params.set("limit", String(EMITTER_MARKER_EMISSIONS_LIMIT));
      return {
        queryKey: [...queryKeys.emissions, "map", params.toString()],
        queryFn: () => listEmissions(params),
      };
    }),
  });

  const emissionsItems = useMemo(
    () => allEmissionsQueries.flatMap((query) => query.data?.items ?? []),
    [allEmissionsQueries],
  );
  const emissionsIsError = allEmissionsQueries.some((query) => query.isError);

  // Task 8: the heatmap's own data — every located point in scope, not just
  // the newest `EMITTER_MARKER_EMISSIONS_LIMIT`-sized page (see module doc
  // comment). Same per-source fan-out as `allEmissionsQueries` above, but
  // hitting the uncapped points endpoint instead.
  const allPointsQueries = useQueries({
    queries: sourceScopeIds.map((sourceId) => {
      const params = buildEmissionsParams({
        dataSourceId: sourceId,
        timeFrom,
        timeTo,
      });
      return {
        queryKey: [...queryKeys.emissions, "map-points", params.toString()],
        queryFn: () => listEmissionPoints(params),
      };
    }),
  });

  const heatmapPoints = useMemo<[number, number][]>(
    () => allPointsQueries.flatMap((query) => query.data?.points ?? []),
    [allPointsQueries],
  );
  // True if ANY selected source's points fetch hit the server's `MAX_POINTS`
  // safety cap — surfaced as a small notice near the map (see the JSX below)
  // rather than silently under-representing the track.
  const anyTruncated = useMemo(
    () => allPointsQueries.some((query) => query.data?.truncated === true),
    [allPointsQueries],
  );

  // Emitters are fetched only to resolve emitter *names* for the Emitters
  // marker layer's labels (`emitterNames` below). Interim `{limit: 500}` cap
  // — `GET /api/emitters` returns a paginated `{items, total}` envelope; 500
  // keeps today's coverage without adding pagination to the map.
  const emittersQuery = useQuery({
    queryKey: queryKeys.emitters,
    queryFn: () => listEmitters({ limit: 500 }),
  });

  // Interim `{limit: 500}` cap, same rationale as the emitters query above.
  const entitiesQuery = useQuery({
    queryKey: queryKeys.entities,
    queryFn: () => listEntities({ limit: 500 }),
  });

  const entityIds = useMemo(
    () => (entitiesQuery.data?.items ?? []).map((entity) => entity.id),
    [entitiesQuery.data],
  );

  // One detail fetch per entity (see module doc comment on why this is
  // reasonable rather than YAGNI-violating scope creep onto the backend).
  const entityDetailQueries = useQueries({
    queries: entityIds.map((id) => ({
      queryKey: [...queryKeys.entities, id],
      queryFn: () => getEntityDetail(id),
    })),
  });

  const entityMarkers = useMemo<EntityMarker[]>(() => {
    const markers: EntityMarker[] = [];
    for (const result of entityDetailQueries) {
      const detail = result.data;
      if (!detail || detail.recent_detections.length === 0) continue;
      const latest = detail.recent_detections.reduce((a, b) =>
        a.observed_at > b.observed_at ? a : b,
      );
      markers.push({
        id: detail.id,
        name: detail.name,
        lon: latest.lon,
        lat: latest.lat,
        observed_at: latest.observed_at,
      });
    }
    return markers;
  }, [entityDetailQueries]);

  // Phase 6 "Emitters" layer: derived client-side from the same
  // "all emissions" items already fetched above, grouped by `emitter_id`
  // (`emitterMarkersFromEmissions`, `components/mapData.ts`) and labeled via
  // this `emittersQuery` fetch — no separate backend request needed.
  const emitterNames = useMemo(() => {
    const names: Record<string, string> = {};
    for (const emitter of emittersQuery.data?.items ?? [])
      names[emitter.id] = emitter.name;
    return names;
  }, [emittersQuery.data]);

  const emitterMarkers = useMemo(
    () => emitterMarkersFromEmissions(emissionsItems, emitterNames),
    [emissionsItems, emitterNames],
  );

  // Full emitter records by id, kept current for the marker click popup. The
  // click handler is registered once on the map (in the init effect) and reads
  // through this ref, so it always sees the latest `emittersQuery` data without
  // needing to re-register on every data change.
  const emittersByIdRef = useRef<Map<string, Emitter>>(new Map());
  useEffect(() => {
    const byId = new Map<string, Emitter>();
    for (const emitter of emittersQuery.data?.items ?? [])
      byId.set(emitter.id, emitter);
    emittersByIdRef.current = byId;
  }, [emittersQuery.data]);

  // A single reused popup instance (created on map load); clicking an emitter
  // marker moves + refills it.
  const popupRef = useRef<Popup | null>(null);

  const zonesQuery = useQuery({
    queryKey: queryKeys.zones,
    queryFn: listZones,
  });

  // Phase 5: GPS fix backing center-on-load + the "Recenter to me" button
  // (see module doc comment).
  const gpsStatusQuery = useQuery({
    queryKey: queryKeys.gpsStatus,
    queryFn: getGpsStatus,
    refetchInterval: GPS_STATUS_REFETCH_MS,
  });
  const hasGpsFix = Boolean(
    gpsStatusQuery.data?.has_fix &&
    gpsStatusQuery.data.lat !== null &&
    gpsStatusQuery.data.lon !== null,
  );
  const hasCenteredOnLoadRef = useRef(false);

  // Latest GPS fix, read by the Auto Track interval so its callback never
  // closes over a stale fix.
  const gpsFixRef = useRef<{ lon: number; lat: number } | null>(null);
  useEffect(() => {
    const gps = gpsStatusQuery.data;
    gpsFixRef.current =
      gps?.has_fix && gps.lat !== null && gps.lon !== null
        ? { lon: gps.lon, lat: gps.lat }
        : null;
  }, [gpsStatusQuery.data]);

  // Map init — runs once. In tests, `maplibre-gl` is mocked
  // (`vi.mock('maplibre-gl', ...)` in `MapView.test.tsx`) so this never
  // touches a real WebGL canvas; jsdom itself is never specially detected
  // here (see module doc comment). The style starts at the `basemap` prop if
  // pinned, else `DEFAULT_BASEMAP_ID` (Satellite) — later basemap switches
  // swap the `BASEMAP_SOURCE_ID` source's tiles in place (see the effect
  // below), never re-create the map or call `setStyle`.
  useEffect(() => {
    if (!containerRef.current) return undefined;

    const map = new maplibregl.Map({
      container: containerRef.current,
      style: buildBasemapStyle(basemap ?? DEFAULT_BASEMAP_ID),
      center: DEFAULT_CENTER,
      zoom: DEFAULT_ZOOM,
      // We render our own attribution caption (bottom-right overlay below)
      // that reflects the *current* basemap — MapLibre's default control
      // wouldn't update on a `setTiles` basemap swap (see `basemapStyles.ts`).
      attributionControl: false,
    });
    mapRef.current = map;
    map.addControl(new maplibregl.NavigationControl(), "top-right");
    const popup = new maplibregl.Popup({
      closeButton: true,
      closeOnClick: true,
      className: "fluxfang-map-popup",
      maxWidth: "260px",
    });
    popupRef.current = popup;

    // Clicking an emitter marker opens a details popup (name/type/identity/
    // last-seen), pulling the full record from `emittersByIdRef`.
    const openEmitterPopup = (event: MapLayerMouseEvent) => {
      const feature = event.features?.[0];
      if (!feature || feature.geometry.type !== "Point") return;
      const [lon, lat] = feature.geometry.coordinates as [number, number];
      const props = feature.properties ?? {};
      const id =
        typeof props.id === "string" ? props.id : String(props.id ?? "");
      const fallbackName = typeof props.name === "string" ? props.name : id;
      const observedAt =
        typeof props.observed_at === "string" ? props.observed_at : null;
      popup
        .setLngLat([lon, lat])
        .setHTML(
          buildEmitterPopupHtml(
            emittersByIdRef.current.get(id),
            fallbackName,
            observedAt,
          ),
        )
        .addTo(map);
    };
    const setPointer = () => {
      map.getCanvas().style.cursor = "pointer";
    };
    const clearPointer = () => {
      map.getCanvas().style.cursor = "";
    };

    map.on("load", () => {
      addLayers(map);
      setStyleLoaded(true);
      // Belt-and-suspenders: if the container hadn't been laid out to its
      // final size when `new Map(...)` measured it, the canvas would be the
      // wrong size (often 0 → blank). A resize here syncs it to the
      // now-laid-out container.
      map.resize();
      // Emitter marker interactivity (registered after the layers exist).
      for (const layerId of EMITTER_LAYER_IDS) {
        map.on("click", layerId, openEmitterPopup);
        map.on("mouseenter", layerId, setPointer);
        map.on("mouseleave", layerId, clearPointer);
      }
    });

    return () => {
      popup.remove();
      popupRef.current = null;
      map.remove();
      mapRef.current = null;
      setStyleLoaded(false);
    };
  }, []);

  // Basemap switch: swap the basemap source's tiles in place, leaving every
  // other source/layer (heatmaps/markers/zones) untouched — see
  // `components/basemapStyles.ts`'s module doc comment for why this is
  // `setTiles`, not `map.setStyle(...)`. Also fires once on initial mount
  // (re-applying `DEFAULT_BASEMAP_ID`'s own tiles) — harmless, since that's
  // already what the map was constructed with.
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<RasterTileSource>(BASEMAP_SOURCE_ID);
    source?.setTiles(basemapOption(basemapId).tiles);
  }, [styleLoaded, basemapId]);

  // Center-on-user-load (Phase 5, see module doc comment): fires exactly
  // once per mount, as soon as both the style has loaded and a GPS fix is
  // known. Guarded by `hasCenteredOnLoadRef` so a later `gpsStatusQuery`
  // refetch (the query polls every `GPS_STATUS_REFETCH_MS`) never yanks the
  // view out from under a user who's since panned elsewhere — only the
  // explicit "Recenter to me" button does that after this initial jump.
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded || hasCenteredOnLoadRef.current) return;
    const gps = gpsStatusQuery.data;
    if (!gps?.has_fix || gps.lat === null || gps.lon === null) return;
    hasCenteredOnLoadRef.current = true;
    map.jumpTo({ center: [gps.lon, gps.lat], zoom: USER_LOCATION_ZOOM });
  }, [styleLoaded, gpsStatusQuery.data]);

  // "Recenter to me" click handler — animated (`flyTo`), unlike the
  // immediate `jumpTo` above, since there's a previous view to transition
  // from this time. Uses whatever fix `gpsStatusQuery` currently has
  // cached; disabled entirely (see the button below) when there's none.
  function handleRecenter(): void {
    const map = mapRef.current;
    const gps = gpsStatusQuery.data;
    if (!map || !gps?.has_fix || gps.lat === null || gps.lon === null) return;
    map.flyTo({ center: [gps.lon, gps.lat], zoom: USER_LOCATION_ZOOM });
  }

  // Auto Track: while enabled, recenter on the latest GPS fix immediately and
  // every 5s — snapping back even after a manual pan (per design). Reads the
  // fix through `gpsFixRef` so the interval callback stays current. Cleared on
  // disable/unmount.
  useEffect(() => {
    if (!autoTrack) return undefined;
    const recenter = () => {
      const map = mapRef.current;
      const fix = gpsFixRef.current;
      if (!map || !fix) return;
      map.flyTo({ center: [fix.lon, fix.lat], zoom: USER_LOCATION_ZOOM });
    };
    recenter();
    const interval = setInterval(recenter, 5000);
    return () => clearInterval(interval);
  }, [autoTrack]);

  // Push fresh source data whenever the underlying queries resolve/change,
  // once the style (and thus the sources) exist. Task 8: fed by
  // `heatmapPoints` (the uncapped points endpoint), not `emissionsItems`
  // (the 500-capped list, which now only backs the emitter markers).
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(EMISSIONS_SOURCE_ID);
    source?.setData(emissionPointsToHeatmapGeoJSON(heatmapPoints));
  }, [styleLoaded, heatmapPoints]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(ENTITIES_SOURCE_ID);
    source?.setData(entitiesToMarkerFeatures(entityMarkers));
  }, [styleLoaded, entityMarkers]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(EMITTERS_SOURCE_ID);
    source?.setData(entitiesToMarkerFeatures(emitterMarkers));
  }, [styleLoaded, emitterMarkers]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(ZONES_SOURCE_ID);
    source?.setData(zonesToCircleGeoJSON(zonesQuery.data ?? []));
  }, [styleLoaded, zonesQuery.data]);

  // Layer-toggle visibility.
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const apply = (layerIds: string[], visible: boolean) => {
      for (const id of layerIds) {
        if (map.getLayer(id))
          map.setLayoutProperty(id, "visibility", visible ? "visible" : "none");
      }
    };
    // The emissions heatmap is always shown now — the Sources group is the
    // sole emission filter (the old "Emissions" category-toggle group was
    // removed as redundant with it).
    apply(HEATMAP_LAYER_IDS, true);
    apply(ENTITY_LAYER_IDS, layerVisibility.entities);
    apply(EMITTER_LAYER_IDS, layerVisibility.emitters);
    apply(ZONE_LAYER_IDS, layerVisibility.zones);
  }, [styleLoaded, layerVisibility]);

  function toggleLayer(layer: keyof LayerVisibility): void {
    setLayerVisibility((prev) => ({ ...prev, [layer]: !prev[layer] }));
  }

  function isSourceSelected(sourceId: string): boolean {
    return sourceSelected[sourceId] ?? false;
  }

  function toggleSource(sourceId: string): void {
    setSourceSelected((prev) => ({
      ...prev,
      [sourceId]: !(prev[sourceId] ?? false),
    }));
  }

  return (
    <div className="flex h-full flex-col gap-3">
      {showControls && (
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Map</h1>
        </div>
      )}

      {showControls && (
        <div className="flex flex-wrap items-start gap-6 rounded border border-slate-800 bg-slate-900/50 p-3">
          {/* Order (left→right): BaseMap → From/To → Layers → Sources. */}
          <div className="flex flex-wrap items-end gap-3">
            {basemap === undefined && (
              <div className="space-y-1">
                <label htmlFor="map-basemap" className={labelClassName}>
                  Basemap
                </label>
                <select
                  id="map-basemap"
                  value={basemapId}
                  onChange={(event) =>
                    setBasemapId(event.target.value as BasemapId)
                  }
                  className={inputClassName}
                >
                  {BASEMAP_OPTIONS.map((option) => (
                    <option key={option.id} value={option.id}>
                      {option.label}
                    </option>
                  ))}
                </select>
                {/* Attribution moved to a map-corner overlay (below) so this
                    caption no longer adds height here and knocks the Basemap
                    control out of alignment with the From/To pickers. */}
              </div>
            )}

            {timeFromProp === undefined && timeToProp === undefined && (
              <>
                <div className="space-y-1">
                  <label htmlFor="map-time-from" className={labelClassName}>
                    From
                  </label>
                  <input
                    id="map-time-from"
                    type="datetime-local"
                    value={timeFromState}
                    onChange={(event) => setTimeFrom(event.target.value)}
                    className={inputClassName}
                  />
                </div>

                <div className="space-y-1">
                  <label htmlFor="map-time-to" className={labelClassName}>
                    To
                  </label>
                  <input
                    id="map-time-to"
                    type="datetime-local"
                    value={timeToState}
                    onChange={(event) => setTimeTo(event.target.value)}
                    className={inputClassName}
                  />
                </div>
              </>
            )}
          </div>

          <fieldset className={groupBoxClassName}>
            <legend className={groupLegendClassName}>Layers</legend>
            <div className={groupCheckboxRowClassName}>
              <label className={checkboxLabelClassName}>
                <input
                  type="checkbox"
                  checked={layerVisibility.zones}
                  onChange={() => toggleLayer("zones")}
                  className={checkboxInputClassName}
                />
                Zones
              </label>
              <label className={checkboxLabelClassName}>
                <input
                  type="checkbox"
                  checked={layerVisibility.entities}
                  onChange={() => toggleLayer("entities")}
                  className={checkboxInputClassName}
                />
                Entities
              </label>
              <label className={checkboxLabelClassName}>
                <input
                  type="checkbox"
                  checked={layerVisibility.emitters}
                  onChange={() => toggleLayer("emitters")}
                  className={checkboxInputClassName}
                />
                Emitters
              </label>
            </div>
          </fieldset>

          <fieldset className={groupBoxClassName}>
            <legend className={groupLegendClassName}>Sources</legend>
            <div className={groupCheckboxRowClassName}>
              <label className={checkboxLabelClassName}>
                <input
                  type="checkbox"
                  checked={allSources}
                  onChange={() => setAllSources((prev) => !prev)}
                  className={checkboxInputClassName}
                />
                All Sources
              </label>
              {(dataSourcesQuery.data ?? [])
                .filter(isEmittingSource)
                .map((source) => (
                  <label key={source.id} className={checkboxLabelClassName}>
                    <input
                      type="checkbox"
                      checked={isSourceSelected(source.id)}
                      disabled={allSources}
                      onChange={() => toggleSource(source.id)}
                      className={checkboxInputClassName}
                    />
                    {source.kind} ({source.interface ?? source.id})
                  </label>
                ))}
            </div>
          </fieldset>

          {emissionsIsError && (
            <p className="text-sm text-red-400">Failed to load emissions.</p>
          )}
        </div>
      )}

      {/* Task 8: a small, non-blocking notice when the points endpoint's
          server-side `MAX_POINTS` cap was hit for any selected source —
          rendered unconditionally (not gated on `showControls`) since the
          Dashboard's embedded map needs it too, not just the `/map` route. */}
      {anyTruncated && (
        <p className="text-xs text-amber-400">
          Showing up to 50,000 points; some older points are hidden.
        </p>
      )}

      {/* The sizing classes (`min-h-[420px] flex-1`) live on the ref'd
          container itself — MapLibre reads its size at construction and only
          auto-resizes on *window* resize, so a container that measures 0 at
          init (which a wrapper + `absolute inset-0` child can, since the child
          is out of flow) leaves the map blank forever. `relative` makes it the
          positioning context for the overlaid recenter button; MapLibre just
          appends its canvas as additional children. */}
      <div
        ref={containerRef}
        data-testid="maplibre-container"
        className="relative min-h-[420px] flex-1 overflow-hidden rounded border border-slate-800"
      >
        {/* Top-left stack: recenter button, then any caller-supplied control
            (Dashboard's Time Range selector). */}
        <div className="absolute left-3 top-3 z-10 flex flex-col items-start gap-2">
          <button
            type="button"
            onClick={handleRecenter}
            disabled={!hasGpsFix}
            title={hasGpsFix ? "Recenter to my location" : "No GPS fix"}
            className="rounded border border-slate-700 bg-slate-900/90 px-2 py-1.5 text-xs font-medium text-slate-200 shadow hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Recenter to me
          </button>
          <button
            type="button"
            onClick={() => setAutoTrack((prev) => !prev)}
            disabled={!hasGpsFix && !autoTrack}
            title={
              hasGpsFix ? "Continuously recenter on my location" : "No GPS fix"
            }
            aria-pressed={autoTrack}
            className={`rounded border px-2 py-1.5 text-xs font-medium shadow disabled:cursor-not-allowed disabled:opacity-50 ${
              autoTrack
                ? "border-amber-500 bg-amber-500/20 text-amber-300"
                : "border-slate-700 bg-slate-900/90 text-slate-200 hover:bg-slate-800"
            }`}
          >
            {autoTrack ? "Auto Track: On" : "Auto Track"}
          </button>
          {overlayTopLeft}
        </div>

        {/* Bottom-left slot (Dashboard's GPS status readout). */}
        {overlayBottomLeft && (
          <div className="absolute bottom-3 left-3 z-10">
            {overlayBottomLeft}
          </div>
        )}

        {/* Basemap attribution — its own map-corner overlay, reflecting the
            current basemap (MapLibre's default control is disabled; see the
            map-init effect). */}
        <div className="pointer-events-none absolute bottom-1 right-1 z-10 rounded bg-slate-900/70 px-1.5 py-0.5 text-[10px] text-slate-400">
          {basemapOption(basemapId).attribution}
        </div>
      </div>
    </div>
  );
}

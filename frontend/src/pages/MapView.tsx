// Task 9.7: Map page — a MapLibre GL map showing an emissions heatmap,
// entity markers, and a zones overlay, with layer toggles + a scoping
// filter row.
//
// Style: a hand-built keyless *raster* style pointing at OpenStreetMap's
// standard tile server (`https://tile.openstreetmap.org/{z}/{x}/{y}.png`),
// not a vector style from a hosted (API-keyed) provider like MapTiler/
// Mapbox — this app has no map-tile API key configured anywhere, and the
// brief calls for "a free raster/vector style ... no API key". NOTE: this
// still needs *runtime internet access* for the browser to fetch OSM tile
// images; the data layers below (heatmap/entities/zones, built from this
// app's own API) render regardless of whether tiles load, they just draw on
// a blank background if OSM is unreachable.
//
// Data sources for the three layers:
//   - Heatmap: `GET /api/emissions` (scoped by this page's own filter row),
//     shaped by `emissionsToHeatmapGeoJSON` (`components/mapData.ts`) —
//     drops emissions with no location (`lon`/`lat` null).
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
//   - Zones: `GET /api/zones`, shaped by `zonesToCircleGeoJSON` into a
//     circle-approximation polygon per zone.
//
// jsdom/test guard: MapLibre GL needs a real WebGL canvas, which jsdom
// doesn't provide. Rather than special-casing the component for tests, the
// test file (`MapView.test.tsx`) `vi.mock('maplibre-gl')`s the whole module
// so `new maplibregl.Map(...)` never touches a real canvas; this component
// itself just does normal effect-based init and trusts that mock in tests.
import { useEffect, useMemo, useRef, useState } from 'react';
import { useQueries, useQuery } from '@tanstack/react-query';
import maplibregl from 'maplibre-gl';
import type { GeoJSONSource, Map as MapLibreMap, StyleSpecification } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
import { queryKeys } from '../api/queryKeys';
import { listEmissions } from '../api/emissions';
import { getEntityDetail, listEntities } from '../api/entities';
import { listZones } from '../api/zones';
import { listDataSources } from '../api/dataSources';
import type { EntityMarker } from '../components/mapData';
import { emissionsToHeatmapGeoJSON, entitiesToMarkerFeatures, zonesToCircleGeoJSON } from '../components/mapData';

/** A keyless raster style: one raster source hitting OSM's standard tile
 * endpoint, one layer drawing it. See module doc comment for the runtime-
 * internet caveat. */
const OSM_RASTER_STYLE: StyleSpecification = {
  version: 8,
  sources: {
    osm: {
      type: 'raster',
      tiles: ['https://tile.openstreetmap.org/{z}/{x}/{y}.png'],
      tileSize: 256,
      attribution: '© OpenStreetMap contributors',
    },
  },
  layers: [{ id: 'osm-tiles', type: 'raster', source: 'osm' }],
};

const EMISSIONS_SOURCE_ID = 'emissions-heatmap-source';
const ENTITIES_SOURCE_ID = 'entity-markers-source';
const ZONES_SOURCE_ID = 'zones-source';

const HEATMAP_LAYER_IDS = ['emissions-heatmap-layer'];
const ENTITY_LAYER_IDS = ['entity-circle-layer', 'entity-label-layer'];
const ZONE_LAYER_IDS = ['zone-fill-layer', 'zone-line-layer', 'zone-label-layer'];

const EMPTY_POINT_FC = { type: 'FeatureCollection' as const, features: [] };
const EMPTY_POLYGON_FC = { type: 'FeatureCollection' as const, features: [] };

interface LayerVisibility {
  heatmap: boolean;
  entities: boolean;
  zones: boolean;
}

const DEFAULT_VISIBILITY: LayerVisibility = { heatmap: true, entities: true, zones: true };

/** Adds every source/layer this page draws, once the style has finished
 * loading. Split out of the init effect so it's obvious this only ever
 * touches a freshly-created map on its `'load'` event. */
function addLayers(map: MapLibreMap): void {
  map.addSource(EMISSIONS_SOURCE_ID, { type: 'geojson', data: EMPTY_POINT_FC });
  map.addLayer({
    id: 'emissions-heatmap-layer',
    type: 'heatmap',
    source: EMISSIONS_SOURCE_ID,
    paint: {
      'heatmap-weight': 1,
      'heatmap-intensity': 1,
      'heatmap-radius': 20,
      'heatmap-opacity': 0.75,
    },
  });

  map.addSource(ENTITIES_SOURCE_ID, { type: 'geojson', data: EMPTY_POINT_FC });
  map.addLayer({
    id: 'entity-circle-layer',
    type: 'circle',
    source: ENTITIES_SOURCE_ID,
    paint: {
      'circle-radius': 6,
      'circle-color': '#f59e0b',
      'circle-stroke-width': 2,
      'circle-stroke-color': '#0f172a',
    },
  });
  map.addLayer({
    id: 'entity-label-layer',
    type: 'symbol',
    source: ENTITIES_SOURCE_ID,
    layout: {
      'text-field': ['get', 'name'],
      'text-size': 12,
      'text-offset': [0, 1.2],
      'text-anchor': 'top',
    },
    paint: { 'text-color': '#f8fafc', 'text-halo-color': '#0f172a', 'text-halo-width': 1 },
  });

  map.addSource(ZONES_SOURCE_ID, { type: 'geojson', data: EMPTY_POLYGON_FC });
  map.addLayer({
    id: 'zone-fill-layer',
    type: 'fill',
    source: ZONES_SOURCE_ID,
    paint: { 'fill-color': '#38bdf8', 'fill-opacity': 0.15 },
  });
  map.addLayer({
    id: 'zone-line-layer',
    type: 'line',
    source: ZONES_SOURCE_ID,
    paint: { 'line-color': '#38bdf8', 'line-width': 2 },
  });
  map.addLayer({
    id: 'zone-label-layer',
    type: 'symbol',
    source: ZONES_SOURCE_ID,
    layout: { 'text-field': ['get', 'name'], 'text-size': 12 },
    paint: { 'text-color': '#38bdf8', 'text-halo-color': '#0f172a', 'text-halo-width': 1 },
  });
}

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';

export default function MapView() {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const [styleLoaded, setStyleLoaded] = useState(false);
  const [visibility, setVisibility] = useState<LayerVisibility>(DEFAULT_VISIBILITY);

  // Filter row: data-source + time range, both optional. Scopes the
  // emissions query only (entities/zones aren't time/source-scoped
  // concepts) — see module doc comment.
  const [dataSourceId, setDataSourceId] = useState('');
  const [timeFrom, setTimeFrom] = useState('');
  const [timeTo, setTimeTo] = useState('');

  const dataSourcesQuery = useQuery({ queryKey: queryKeys.dataSources, queryFn: listDataSources });

  const emissionsParams = useMemo(() => {
    const params = new URLSearchParams();
    // 500 is a generous cap for a single map view — see task brief ("use
    // GET /api/emissions?limit=500 and filter to those with non-null
    // lon/lat client-side"). Located-only filtering happens in
    // `emissionsToHeatmapGeoJSON`, not here.
    params.set('limit', '500');
    if (dataSourceId.length > 0) params.set('data_source_id', dataSourceId);
    if (timeFrom.length > 0) params.set('time_from', new Date(timeFrom).toISOString());
    if (timeTo.length > 0) params.set('time_to', new Date(timeTo).toISOString());
    return params;
  }, [dataSourceId, timeFrom, timeTo]);

  const emissionsQuery = useQuery({
    queryKey: [...queryKeys.emissions, 'map', emissionsParams.toString()],
    queryFn: () => listEmissions(emissionsParams),
  });

  const entitiesQuery = useQuery({ queryKey: queryKeys.entities, queryFn: listEntities });

  const entityIds = useMemo(() => (entitiesQuery.data ?? []).map((entity) => entity.id), [entitiesQuery.data]);

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
      const latest = detail.recent_detections.reduce((a, b) => (a.observed_at > b.observed_at ? a : b));
      markers.push({ id: detail.id, name: detail.name, lon: latest.lon, lat: latest.lat, observed_at: latest.observed_at });
    }
    return markers;
  }, [entityDetailQueries]);

  const zonesQuery = useQuery({ queryKey: queryKeys.zones, queryFn: listZones });

  // Map init — runs once. In tests, `maplibre-gl` is mocked
  // (`vi.mock('maplibre-gl', ...)` in `MapView.test.tsx`) so this never
  // touches a real WebGL canvas; jsdom itself is never specially detected
  // here (see module doc comment).
  useEffect(() => {
    if (!containerRef.current) return undefined;

    const map = new maplibregl.Map({
      container: containerRef.current,
      style: OSM_RASTER_STYLE,
      center: [0, 0],
      zoom: 2,
    });
    mapRef.current = map;
    map.addControl(new maplibregl.NavigationControl(), 'top-right');
    map.on('load', () => {
      addLayers(map);
      setStyleLoaded(true);
    });

    return () => {
      map.remove();
      mapRef.current = null;
      setStyleLoaded(false);
    };
  }, []);

  // Push fresh source data whenever the underlying queries resolve/change,
  // once the style (and thus the sources) exist.
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(EMISSIONS_SOURCE_ID);
    source?.setData(emissionsToHeatmapGeoJSON(emissionsQuery.data?.items ?? []));
  }, [styleLoaded, emissionsQuery.data]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(ENTITIES_SOURCE_ID);
    source?.setData(entitiesToMarkerFeatures(entityMarkers));
  }, [styleLoaded, entityMarkers]);

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
        if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', visible ? 'visible' : 'none');
      }
    };
    apply(HEATMAP_LAYER_IDS, visibility.heatmap);
    apply(ENTITY_LAYER_IDS, visibility.entities);
    apply(ZONE_LAYER_IDS, visibility.zones);
  }, [styleLoaded, visibility]);

  function toggle(layer: keyof LayerVisibility): void {
    setVisibility((prev) => ({ ...prev, [layer]: !prev[layer] }));
  }

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Map</h1>
      </div>

      <div className="flex flex-wrap items-end gap-4 rounded border border-slate-800 bg-slate-900/50 p-3">
        <div className="flex items-center gap-4">
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={visibility.heatmap}
              onChange={() => toggle('heatmap')}
              className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
            />
            Heatmap
          </label>
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={visibility.entities}
              onChange={() => toggle('entities')}
              className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
            />
            Entities
          </label>
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={visibility.zones}
              onChange={() => toggle('zones')}
              className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
            />
            Zones
          </label>
        </div>

        <div className="flex flex-wrap items-end gap-3">
          <div className="space-y-1">
            <label htmlFor="map-data-source" className={labelClassName}>
              Data source
            </label>
            <select
              id="map-data-source"
              value={dataSourceId}
              onChange={(event) => setDataSourceId(event.target.value)}
              className={inputClassName}
            >
              <option value="">All sources</option>
              {(dataSourcesQuery.data ?? []).map((source) => (
                <option key={source.id} value={source.id}>
                  {source.kind} ({source.interface ?? source.id})
                </option>
              ))}
            </select>
          </div>

          <div className="space-y-1">
            <label htmlFor="map-time-from" className={labelClassName}>
              From
            </label>
            <input
              id="map-time-from"
              type="datetime-local"
              value={timeFrom}
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
              value={timeTo}
              onChange={(event) => setTimeTo(event.target.value)}
              className={inputClassName}
            />
          </div>
        </div>

        {emissionsQuery.isError && <p className="text-sm text-red-400">Failed to load emissions.</p>}
      </div>

      <div
        ref={containerRef}
        data-testid="maplibre-container"
        className="min-h-[420px] flex-1 overflow-hidden rounded border border-slate-800"
      />
    </div>
  );
}

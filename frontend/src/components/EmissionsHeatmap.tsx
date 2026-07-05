// Task C (emitter auto-classification design doc, "Map (reframed, scoped
// heatmaps)"): a small, embeddable MapLibre heatmap — reused by both
// `Emitters.tsx`'s `EmitterDetail` ("where this emitter has been heard") and
// `Entities.tsx`'s `EntityDetail` ("where this entity's emitters have been
// heard"), rather than each rebuilding its own scoped map.
//
// Deliberately dumb: it takes already-located `points` (the caller has
// already fetched + filtered to non-null `lon`/`lat`, or the data — like an
// entity's `recent_detections` — was already guaranteed located) and just
// draws them. It does no fetching of its own.
//
// Same style/init pattern as the overview `MapView.tsx` (shared
// `OSM_RASTER_STYLE`, same jsdom/test guard: `maplibre-gl` is mocked
// wholesale in tests so `new maplibregl.Map(...)` never touches a real
// WebGL canvas jsdom doesn't have). Unlike `MapView`, an empty `points` list
// renders a plain empty-state message instead of an empty map — a detail
// heatmap with nothing to show is a much more common case here (a
// brand-new emitter/entity with no located detections yet) than on the
// overview page, so skipping the map entirely avoids an oddly-blank
// zoomed-out globe.
import { useEffect, useRef, useState } from 'react';
import maplibregl from 'maplibre-gl';
import type { GeoJSONSource, LngLatBoundsLike, Map as MapLibreMap } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
import { OSM_RASTER_STYLE } from './osmRasterStyle';
import { pointsToHeatmapGeoJSON } from './mapData';
import type { HeatmapPoint } from './mapData';

const SOURCE_ID = 'emissions-heatmap-compact-source';
const LAYER_ID = 'emissions-heatmap-compact-layer';

export interface EmissionsHeatmapProps {
  points: HeatmapPoint[];
  /** CSS height of the map container — a compact embed defaults smaller
   * than the overview page's near-full-height map. */
  height?: string;
  className?: string;
}

/** A bounding box around every point, for `fitBounds` — `null` for an empty
 * list (nothing to fit to; callers should have already routed to the empty
 * state in that case). */
function boundsFor(points: HeatmapPoint[]): LngLatBoundsLike | null {
  if (points.length === 0) return null;
  let minLon = points[0].lon;
  let maxLon = points[0].lon;
  let minLat = points[0].lat;
  let maxLat = points[0].lat;
  for (const point of points) {
    minLon = Math.min(minLon, point.lon);
    maxLon = Math.max(maxLon, point.lon);
    minLat = Math.min(minLat, point.lat);
    maxLat = Math.max(maxLat, point.lat);
  }
  return [
    [minLon, minLat],
    [maxLon, maxLat],
  ];
}

export default function EmissionsHeatmap({ points, height = '260px', className = '' }: EmissionsHeatmapProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const [styleLoaded, setStyleLoaded] = useState(false);
  const hasPoints = points.length > 0;

  // Map init — only while there are points to show (see module doc comment
  // for why an empty list skips the map entirely rather than rendering a
  // blank zoomed-out one). Re-runs only on the empty <-> non-empty
  // transition, not on every `points` change — see the data-push effect
  // below for how later updates reach an already-created map.
  useEffect(() => {
    if (!hasPoints || !containerRef.current) return undefined;

    const map = new maplibregl.Map({
      container: containerRef.current,
      style: OSM_RASTER_STYLE,
      center: [points[0].lon, points[0].lat],
      zoom: 12,
    });
    mapRef.current = map;
    map.addControl(new maplibregl.NavigationControl(), 'top-right');
    map.on('load', () => {
      map.addSource(SOURCE_ID, { type: 'geojson', data: pointsToHeatmapGeoJSON(points) });
      map.addLayer({
        id: LAYER_ID,
        type: 'heatmap',
        source: SOURCE_ID,
        paint: {
          'heatmap-weight': 1,
          'heatmap-intensity': 1,
          'heatmap-radius': 20,
          'heatmap-opacity': 0.75,
        },
      });
      const bounds = boundsFor(points);
      if (bounds) map.fitBounds(bounds, { padding: 40, maxZoom: 16, duration: 0 });
      setStyleLoaded(true);
    });

    return () => {
      map.remove();
      mapRef.current = null;
      setStyleLoaded(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only the empty<->non-empty transition should recreate the map; see comment above.
  }, [hasPoints]);

  // Push updated point data (and re-fit bounds) into an already-created map
  // whenever `points` itself changes (e.g. a refetch bringing in a newly
  // located emission), without tearing the map down.
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(SOURCE_ID);
    source?.setData(pointsToHeatmapGeoJSON(points));
    const bounds = boundsFor(points);
    if (bounds) map.fitBounds(bounds, { padding: 40, maxZoom: 16, duration: 0 });
  }, [styleLoaded, points]);

  if (!hasPoints) {
    return (
      <div
        data-testid="emissions-heatmap-empty"
        className={`flex items-center justify-center rounded border border-slate-800 bg-slate-900/40 text-sm text-slate-500 ${className}`}
        style={{ height }}
      >
        No located detections yet.
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      data-testid="emissions-heatmap-container"
      className={`overflow-hidden rounded border border-slate-800 ${className}`}
      style={{ height }}
    />
  );
}

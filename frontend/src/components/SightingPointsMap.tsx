// A compact MapLibre map that plots each of an emitter's detections as a
// discrete circle colored by signal strength — the Co-Travel Details view's
// "where it was heard, and how strongly." Same init/empty-state/test-guard
// pattern as EmissionsHeatmap (maplibre-gl mocked wholesale in tests).
import { useEffect, useRef, useState } from 'react';
import maplibregl from 'maplibre-gl';
import type { GeoJSONSource, LngLatBoundsLike, Map as MapLibreMap } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
import { OSM_RASTER_STYLE } from './osmRasterStyle';
import { sightingPointsToGeoJSON, type SightingPoint } from './mapData';

const SOURCE_ID = 'cotravel-sightings-source';
const LAYER_ID = 'cotravel-sightings-layer';

export interface SightingPointsMapProps {
  points: SightingPoint[];
  height?: string;
  className?: string;
}

function boundsFor(points: SightingPoint[]): LngLatBoundsLike | null {
  if (points.length === 0) return null;
  let minLon = points[0].lon;
  let maxLon = points[0].lon;
  let minLat = points[0].lat;
  let maxLat = points[0].lat;
  for (const p of points) {
    minLon = Math.min(minLon, p.lon);
    maxLon = Math.max(maxLon, p.lon);
    minLat = Math.min(minLat, p.lat);
    maxLat = Math.max(maxLat, p.lat);
  }
  return [
    [minLon, minLat],
    [maxLon, maxLat],
  ];
}

export default function SightingPointsMap({
  points,
  height = '220px',
  className = '',
}: SightingPointsMapProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const [styleLoaded, setStyleLoaded] = useState(false);
  const hasPoints = points.length > 0;

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
      map.addSource(SOURCE_ID, { type: 'geojson', data: sightingPointsToGeoJSON(points) });
      map.addLayer({
        id: LAYER_ID,
        type: 'circle',
        source: SOURCE_ID,
        paint: {
          'circle-radius': 5,
          'circle-opacity': 0.85,
          'circle-stroke-width': 1,
          'circle-stroke-color': '#0f172a',
          // Color by RSSI: weak (-100) red -> mid (-70) amber -> strong (-40)
          // green; a null RSSI falls back to slate gray.
          'circle-color': [
            'case',
            ['==', ['get', 'rssi'], null],
            '#64748b',
            [
              'interpolate',
              ['linear'],
              ['get', 'rssi'],
              -100,
              '#ef4444',
              -70,
              '#f59e0b',
              -40,
              '#22c55e',
            ],
          ],
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- recreate only on empty<->non-empty transition.
  }, [hasPoints]);

  const pointsSignature = points.map((p) => `${p.lon},${p.lat}`).join('|');

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !styleLoaded) return;
    const source = map.getSource<GeoJSONSource>(SOURCE_ID);
    source?.setData(sightingPointsToGeoJSON(points));
    const bounds = boundsFor(points);
    if (bounds) map.fitBounds(bounds, { padding: 40, maxZoom: 16, duration: 0 });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- re-fit only when the point coordinates change, not on every render.
  }, [styleLoaded, pointsSignature]);

  if (!hasPoints) {
    return (
      <div
        data-testid="sighting-points-empty"
        className={`flex items-center justify-center rounded border border-slate-800 bg-slate-900/40 text-sm text-slate-500 ${className}`}
        style={{ height }}
      >
        No located detections in this window.
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      data-testid="sighting-points-container"
      className={`overflow-hidden rounded border border-slate-800 ${className}`}
      style={{ height }}
    />
  );
}

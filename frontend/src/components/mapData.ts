// Pure data-shaping helpers for Task 9.7's Map page (`pages/MapView.tsx`).
// Split out from the component so these can be unit-tested without touching
// MapLibre GL / a real canvas (see that file's module doc comment for why —
// jsdom has no WebGL context).
//
// Each function below turns one of this app's already-typed API shapes
// (`Emission`, an entity's last-known location, `Zone`) into a GeoJSON
// structure MapLibre's `addSource({ type: 'geojson', data })` can consume
// directly.
import type { Feature, FeatureCollection, Point, Polygon } from 'geojson';
import type { Emission } from '../api/emissions';
import type { Zone } from '../api/zones';

/** A bare located point — what `EmissionsHeatmap.tsx` (Task C's reusable
 * per-emitter/per-entity detail heatmap) takes: its callers have already
 * filtered/derived located points out of their own source data (an
 * emitter's `GET /api/emissions?emitter_id=…` items, or an entity's already-
 * located `recent_detections`), so this shaper doesn't need — and shouldn't
 * assume — the full `Emission` shape. */
export interface HeatmapPoint {
  lon: number;
  lat: number;
}

/**
 * `points` -> a GeoJSON `FeatureCollection<Point>` for a compact heatmap
 * layer. No filtering (every point is assumed already-located) and no
 * properties beyond the geometry — this is deliberately the minimal shape
 * `EmissionsHeatmap` needs, not a re-fit of a richer per-feature-properties
 * shape.
 */
export function pointsToHeatmapGeoJSON(points: HeatmapPoint[]): FeatureCollection<Point, Record<string, never>> {
  return {
    type: 'FeatureCollection',
    features: points.map((point) => ({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [point.lon, point.lat] },
      properties: {},
    })),
  };
}

/**
 * `points` (raw `[lon, lat]` pairs, as returned by `GET /api/emissions/points`
 * — see `api/emissions.ts`'s `EmissionPoints.points`) -> a GeoJSON
 * `FeatureCollection<Point>` for the Map/Dashboard heatmap layer
 * (`MapView.tsx`), which is fed by the uncapped points endpoint instead of
 * the 500-capped `GET /api/emissions` list.
 *
 * Deliberately distinct from `pointsToHeatmapGeoJSON` above — that one takes
 * `HeatmapPoint` (`{lon, lat}`) objects, the shape `EmissionsHeatmap.tsx`'s
 * callers already have on hand; this one takes the points endpoint's raw
 * tuples directly, so `MapView` doesn't need to re-map every point into an
 * object just to shape it. Same conventions otherwise: no filtering (every
 * point the endpoint returns is already located) and no properties beyond
 * the geometry.
 */
export function emissionPointsToHeatmapGeoJSON(
  points: [number, number][],
): FeatureCollection<Point, Record<string, never>> {
  return {
    type: 'FeatureCollection',
    features: points.map(([lon, lat]) => ({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [lon, lat] },
      properties: {},
    })),
  };
}

/** A located detection carrying its signal strength — the Co-Travel Details
 * map plots one circle per point, colored by `signal_strength`. */
export interface SightingPoint {
  lon: number;
  lat: number;
  signal_strength: number | null;
}

export interface SightingPointProperties {
  rssi: number | null;
}

/** `points` -> a GeoJSON `FeatureCollection<Point>` whose features each carry
 * an `rssi` property, so a MapLibre circle layer can color each detection by
 * signal strength. No filtering (callers pass already-located points). */
export function sightingPointsToGeoJSON(
  points: SightingPoint[],
): FeatureCollection<Point, SightingPointProperties> {
  return {
    type: 'FeatureCollection',
    features: points.map((p) => ({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [p.lon, p.lat] },
      properties: { rssi: p.signal_strength },
    })),
  };
}

/** One entity's last known location — the caller (`MapView`) derives this
 * per entity from `GET /api/entities/:id`'s `recent_detections` (see that
 * component's doc comment for exactly how "last" is chosen), so this module
 * doesn't need to know about `EntityDetail`/`RecentDetection` shapes at all. */
export interface EntityMarker {
  id: string;
  name: string;
  lon: number;
  lat: number;
  /** ISO timestamp of the detection this marker is placed at, shown in a
   * tooltip so "last location" doesn't read as live/current. */
  observed_at: string;
}

export interface EntityPointProperties {
  id: string;
  name: string;
  observed_at: string;
}

/** `entityMarkers` -> a GeoJSON `FeatureCollection<Point>` for the
 * entity-marker layer, one feature per marker (already-resolved locations —
 * this function does no filtering/derivation of its own). */
export function entitiesToMarkerFeatures(
  entityMarkers: EntityMarker[],
): FeatureCollection<Point, EntityPointProperties> {
  return {
    type: 'FeatureCollection',
    features: entityMarkers.map((marker) => ({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [marker.lon, marker.lat] },
      properties: { id: marker.id, name: marker.name, observed_at: marker.observed_at },
    })),
  };
}

/** Phase 6 (Map page redesign, "Layers" group's Emitters toggle): one marker
 * per emitter, placed at that emitter's most-recent LOCATED emission.
 * Structurally identical to `EntityMarker` (`id`/`name`/`lon`/`lat`/
 * `observed_at`), so it reuses `entitiesToMarkerFeatures` for the GeoJSON
 * shaping rather than needing its own — only the grouping-by-emitter-id
 * (`emitterMarkersFromEmissions` below) is new. */
export type EmitterMarker = EntityMarker;

/**
 * `emissions` -> one `EmitterMarker` per distinct `emitter_id` present,
 * placed at that emitter's most-recent (by `observed_at`) LOCATED emission.
 * Emissions with no location (`lon`/`lat` null) or no `emitter_id` (not yet
 * assigned to an emitter) are dropped — same located-only convention as
 * the other heatmap shapers.
 *
 * `emitterNames` maps `emitter_id` -> display name (from `GET /api/emitters`,
 * the caller's own `listEmitters` fetch); an emitter id missing from it
 * (shouldn't normally happen — every `emitter_id` on an emission should
 * belong to a known emitter, but defensively) falls back to showing the raw
 * id rather than crashing or rendering a blank label.
 */
export function emitterMarkersFromEmissions(
  emissions: Emission[],
  emitterNames: Record<string, string>,
): EmitterMarker[] {
  const latestByEmitter = new Map<string, Emission & { lon: number; lat: number }>();

  for (const emission of emissions) {
    if (emission.lon === null || emission.lat === null) continue;
    if (!emission.emitter_id) continue;
    const located = emission as Emission & { lon: number; lat: number };
    const existing = latestByEmitter.get(emission.emitter_id);
    if (!existing || located.observed_at > existing.observed_at) {
      latestByEmitter.set(emission.emitter_id, located);
    }
  }

  const markers: EmitterMarker[] = [];
  for (const [emitterId, emission] of latestByEmitter) {
    markers.push({
      id: emitterId,
      name: emitterNames[emitterId] ?? emitterId,
      lon: emission.lon,
      lat: emission.lat,
      observed_at: emission.observed_at,
    });
  }
  return markers;
}

export interface ZonePolygonProperties {
  id: string;
  name: string;
  radius_m: number;
}

/** Number of points around each zone's circle-polygon ring (plus the closing
 * point that repeats the first). 64 is plenty smooth at the zoom levels this
 * app cares about (tens-to-hundreds of meters) while keeping the geometry
 * small. */
const CIRCLE_SEGMENTS = 64;

/** Meters per degree of latitude — constant (WGS84 mean radius), used both
 * for the north/south offset directly and to derive the longitude's
 * meters-per-degree at a given latitude (which shrinks toward the poles by
 * `cos(latitude)`). Matches the equirectangular approximation the backend's
 * own bbox/distance math uses at this scale (zones are neighborhood-sized,
 * not planet-spanning, so this approximation's error is negligible). */
const EARTH_RADIUS_M = 6_371_000;
const METERS_PER_DEGREE_LAT = (Math.PI / 180) * EARTH_RADIUS_M;

/**
 * One `zone` -> a GeoJSON `Feature<Polygon>` approximating its circle
 * (`radius_m` around `[lon, lat]`) as a `CIRCLE_SEGMENTS`-sided ring, closed
 * (first coordinate repeated as the last, per the GeoJSON `Polygon` spec).
 */
export function zoneToCircleFeature(zone: Zone): Feature<Polygon, ZonePolygonProperties> {
  const metersPerDegreeLon = METERS_PER_DEGREE_LAT * Math.cos((zone.lat * Math.PI) / 180);

  const ring: [number, number][] = [];
  for (let i = 0; i <= CIRCLE_SEGMENTS; i++) {
    const angle = (i / CIRCLE_SEGMENTS) * 2 * Math.PI;
    const dLon = (zone.radius_m * Math.cos(angle)) / metersPerDegreeLon;
    const dLat = (zone.radius_m * Math.sin(angle)) / METERS_PER_DEGREE_LAT;
    ring.push([zone.lon + dLon, zone.lat + dLat]);
  }

  return {
    type: 'Feature',
    geometry: { type: 'Polygon', coordinates: [ring] },
    properties: { id: zone.id, name: zone.name, radius_m: zone.radius_m },
  };
}

/** `zones` -> a GeoJSON `FeatureCollection<Polygon>` for the zones overlay's
 * fill+line layers (one `zoneToCircleFeature` per zone). */
export function zonesToCircleGeoJSON(zones: Zone[]): FeatureCollection<Polygon, ZonePolygonProperties> {
  return { type: 'FeatureCollection', features: zones.map(zoneToCircleFeature) };
}

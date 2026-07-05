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

/** Properties carried by each heatmap point feature — enough to show in a
 * popup/tooltip later, without pulling the whole `Emission` (and its
 * kind-dependent `payload`) into the GL source. */
export interface EmissionPointProperties {
  id: string;
  kind: string;
  observed_at: string;
  emitter_id: string | null;
}

/**
 * `emissions` -> a GeoJSON `FeatureCollection<Point>` for the heatmap layer.
 *
 * Only emissions with a non-null `lon`/`lat` (see `Emission`'s doc comment —
 * `None` when PostGIS has no location for that row) produce a feature;
 * others are silently dropped, so `result.features.length` always equals the
 * count of *located* emissions in the input, never `emissions.length`.
 */
export function emissionsToHeatmapGeoJSON(
  emissions: Emission[],
): FeatureCollection<Point, EmissionPointProperties> {
  const features: Feature<Point, EmissionPointProperties>[] = [];

  for (const emission of emissions) {
    if (emission.lon === null || emission.lat === null) continue;
    features.push({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [emission.lon, emission.lat] },
      properties: {
        id: emission.id,
        kind: emission.kind,
        observed_at: emission.observed_at,
        emitter_id: emission.emitter_id,
      },
    });
  }

  return { type: 'FeatureCollection', features };
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

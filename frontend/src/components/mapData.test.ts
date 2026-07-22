// Unit tests for the pure data-shaping helpers in `mapData.ts` — the GeoJSON
// shapers (points/emitter-markers/entity-markers) and `zoneToCircleFeature`
// (a closed polygon whose ring sits ~`radius_m` from the center). These are
// tested here instead of GL rendering — see `mapData.ts`'s module doc comment
// for why (jsdom has no WebGL context).
import { describe, expect, it, test } from 'vitest';
import {
  emissionPointsToHeatmapGeoJSON,
  emitterMarkers,
  emitterMarkersFromEmissions,
  emitterUncertaintyCirclesGeoJSON,
  entitiesToMarkerFeatures,
  pointsToHeatmapGeoJSON,
  sightingPointsToGeoJSON,
  zoneToCircleFeature,
  zonesToCircleGeoJSON,
} from './mapData';
import type { Emission } from '../api/emissions';
import type { Emitter } from '../api/emitters';
import type { Zone } from '../api/zones';

function makeEmitter(overrides: Partial<Emitter>): Emitter {
  return {
    id: 'emitter-1',
    name: 'Emitter One',
    type: null,
    emitter_type: null,
    attributes: {},
    match_enabled: true,
    type_label: null,
    category: null,
    entity_id: null,
    match_criteria: {},
    first_seen_at: null,
    last_seen_at: null,
    created_at: '2026-07-01T00:00:00Z',
    emission_count: 0,
    estimate: null,
    ...overrides,
  };
}

function makeEmission(overrides: Partial<Emission>): Emission {
  return {
    id: 'em-1',
    data_source_id: 'ds-1',
    emitter_id: null,
    session_id: null,
    observed_at: '2026-07-01T00:00:00Z',
    signal_strength: -50,
    lon: 2.5,
    lat: 1.5,
    kind: 'wifi',
    payload: {},
    ...overrides,
  };
}

test('entitiesToMarkerFeatures places one point feature per marker at its last location', () => {
  const features = entitiesToMarkerFeatures([
    { id: 'entity-1', name: 'Bob', lon: 2.5, lat: 1.5, observed_at: '2026-07-04T12:00:00Z' },
    { id: 'entity-2', name: 'Alice', lon: -1.1, lat: 5.5, observed_at: '2026-07-03T09:00:00Z' },
  ]);

  expect(features.features).toHaveLength(2);
  expect(features.features[0].geometry.coordinates).toEqual([2.5, 1.5]);
  expect(features.features[0].properties).toEqual({
    id: 'entity-1',
    name: 'Bob',
    observed_at: '2026-07-04T12:00:00Z',
  });
  expect(features.features[1].geometry.coordinates).toEqual([-1.1, 5.5]);
});

test('pointsToHeatmapGeoJSON turns bare {lon,lat} points into one Point feature each, no filtering', () => {
  const geojson = pointsToHeatmapGeoJSON([
    { lon: 2.5, lat: 1.5 },
    { lon: -1.1, lat: 5.5 },
  ]);

  expect(geojson.type).toBe('FeatureCollection');
  expect(geojson.features).toHaveLength(2);
  expect(geojson.features[0].geometry).toEqual({ type: 'Point', coordinates: [2.5, 1.5] });
  expect(geojson.features[1].geometry).toEqual({ type: 'Point', coordinates: [-1.1, 5.5] });
});

test('pointsToHeatmapGeoJSON on an empty list yields an empty FeatureCollection', () => {
  const geojson = pointsToHeatmapGeoJSON([]);
  expect(geojson).toEqual({ type: 'FeatureCollection', features: [] });
});

test('emissionPointsToHeatmapGeoJSON maps raw [lon,lat] pairs to GeoJSON point features', () => {
  const fc = emissionPointsToHeatmapGeoJSON([
    [-122.4, 37.7],
    [-122.5, 37.8],
  ]);

  expect(fc.type).toBe('FeatureCollection');
  expect(fc.features).toHaveLength(2);
  expect(fc.features[0].geometry).toEqual({ type: 'Point', coordinates: [-122.4, 37.7] });
  expect(fc.features[1].geometry).toEqual({ type: 'Point', coordinates: [-122.5, 37.8] });
});

test('emissionPointsToHeatmapGeoJSON on an empty list yields an empty FeatureCollection', () => {
  const fc = emissionPointsToHeatmapGeoJSON([]);
  expect(fc).toEqual({ type: 'FeatureCollection', features: [] });
});

test('emitterMarkersFromEmissions groups located emissions by emitter_id and keeps only the latest per emitter', () => {
  const emissions: Emission[] = [
    makeEmission({ id: 'em-1', emitter_id: 'emitter-1', lon: 2.5, lat: 1.5, observed_at: '2026-07-01T00:00:00Z' }),
    // A later observation for the same emitter, at a different location — this one should win.
    makeEmission({ id: 'em-2', emitter_id: 'emitter-1', lon: 2.6, lat: 1.6, observed_at: '2026-07-03T00:00:00Z' }),
    makeEmission({ id: 'em-3', emitter_id: 'emitter-2', lon: -1.1, lat: 5.5, observed_at: '2026-07-02T00:00:00Z' }),
    // No location — dropped entirely, even though it's for a known emitter.
    makeEmission({ id: 'em-4', emitter_id: 'emitter-1', lon: null, lat: null, observed_at: '2026-07-04T00:00:00Z' }),
    // No emitter_id — dropped (nothing to group it under).
    makeEmission({ id: 'em-5', emitter_id: null, lon: 3.3, lat: 3.3, observed_at: '2026-07-01T00:00:00Z' }),
  ];

  const markers = emitterMarkersFromEmissions(emissions, { 'emitter-1': 'AP One', 'emitter-2': 'Client Two' });

  expect(markers).toHaveLength(2);
  const byId = new Map(markers.map((m) => [m.id, m]));
  expect(byId.get('emitter-1')).toEqual({
    id: 'emitter-1',
    name: 'AP One',
    lon: 2.6,
    lat: 1.6,
    observed_at: '2026-07-03T00:00:00Z',
  });
  expect(byId.get('emitter-2')).toEqual({
    id: 'emitter-2',
    name: 'Client Two',
    lon: -1.1,
    lat: 5.5,
    observed_at: '2026-07-02T00:00:00Z',
  });
});

test('emitterMarkersFromEmissions falls back to the emitter id as the label when no name is known', () => {
  const emissions: Emission[] = [makeEmission({ id: 'em-1', emitter_id: 'emitter-9', lon: 0, lat: 0 })];
  const markers = emitterMarkersFromEmissions(emissions, {});
  expect(markers).toEqual([{ id: 'emitter-9', name: 'emitter-9', lon: 0, lat: 0, observed_at: '2026-07-01T00:00:00Z' }]);
});

describe('emitterMarkers', () => {
  it('places an emitter WITH an estimate at estimate.lon/lat carrying uncertaintyM', () => {
    const emitters: Emitter[] = [
      makeEmitter({
        id: 'emitter-1',
        name: 'Localized AP',
        // A stale located emission at a *different* spot — the estimate wins.
        last_seen_at: '2026-07-03T00:00:00Z',
        estimate: {
          lon: 10.5,
          lat: -3.25,
          uncertainty_m: 75,
          bin_count: 12,
          updated_at: '2026-07-05T00:00:00Z',
        },
      }),
    ];
    const emissions: Emission[] = [
      makeEmission({ id: 'em-1', emitter_id: 'emitter-1', lon: 2.5, lat: 1.5, observed_at: '2026-07-03T00:00:00Z' }),
    ];

    const markers = emitterMarkers(emitters, emissions, { 'emitter-1': 'Localized AP' });

    expect(markers).toHaveLength(1);
    expect(markers[0]).toEqual({
      id: 'emitter-1',
      name: 'Localized AP',
      lon: 10.5,
      lat: -3.25,
      observed_at: '2026-07-05T00:00:00Z',
      uncertaintyM: 75,
    });
  });

  it('falls back to the latest located emission (no uncertaintyM) for an emitter WITHOUT an estimate', () => {
    const emitters: Emitter[] = [makeEmitter({ id: 'emitter-2', name: 'Unlocalized', estimate: null })];
    const emissions: Emission[] = [
      makeEmission({ id: 'em-1', emitter_id: 'emitter-2', lon: 2.5, lat: 1.5, observed_at: '2026-07-01T00:00:00Z' }),
      makeEmission({ id: 'em-2', emitter_id: 'emitter-2', lon: 2.6, lat: 1.6, observed_at: '2026-07-04T00:00:00Z' }),
    ];

    const markers = emitterMarkers(emitters, emissions, { 'emitter-2': 'Unlocalized' });

    expect(markers).toHaveLength(1);
    expect(markers[0]).toEqual({
      id: 'emitter-2',
      name: 'Unlocalized',
      lon: 2.6,
      lat: 1.6,
      observed_at: '2026-07-04T00:00:00Z',
    });
    expect(markers[0].uncertaintyM).toBeUndefined();
  });

  it('mixes estimate-placed and emission-fallback markers, ignoring emissions of estimate-placed emitters', () => {
    const emitters: Emitter[] = [
      makeEmitter({
        id: 'emitter-1',
        estimate: { lon: 10, lat: 10, uncertainty_m: 40, bin_count: 5, updated_at: null },
        last_seen_at: '2026-07-02T00:00:00Z',
      }),
      makeEmitter({ id: 'emitter-2', estimate: null }),
    ];
    const emissions: Emission[] = [
      // Belongs to the estimate-placed emitter — must NOT produce a second marker.
      makeEmission({ id: 'em-1', emitter_id: 'emitter-1', lon: 1, lat: 1, observed_at: '2026-07-01T00:00:00Z' }),
      makeEmission({ id: 'em-2', emitter_id: 'emitter-2', lon: 2, lat: 2, observed_at: '2026-07-01T00:00:00Z' }),
    ];

    const markers = emitterMarkers(emitters, emissions, {});
    const byId = new Map(markers.map((m) => [m.id, m]));

    expect(markers).toHaveLength(2);
    expect(byId.get('emitter-1')).toEqual({ id: 'emitter-1', name: 'Emitter One', lon: 10, lat: 10, observed_at: '2026-07-02T00:00:00Z', uncertaintyM: 40 });
    expect(byId.get('emitter-2')).toMatchObject({ id: 'emitter-2', lon: 2, lat: 2 });
    expect(byId.get('emitter-2')?.uncertaintyM).toBeUndefined();
  });
});

describe('emitterUncertaintyCirclesGeoJSON', () => {
  it('emits one closed polygon (~uncertaintyM out) per marker that has a radius, skipping the rest', () => {
    const collection = emitterUncertaintyCirclesGeoJSON([
      { id: 'emitter-1', name: 'A', lon: 2.5, lat: 1.5, observed_at: '2026-07-05T00:00:00Z', uncertaintyM: 120 },
      // No uncertaintyM (a fallback marker) — skipped.
      { id: 'emitter-2', name: 'B', lon: -1.1, lat: 5.5, observed_at: '2026-07-04T00:00:00Z' },
    ]);

    expect(collection.type).toBe('FeatureCollection');
    expect(collection.features).toHaveLength(1);
    const feature = collection.features[0];
    expect(feature.geometry.type).toBe('Polygon');
    expect(feature.properties).toEqual({ id: 'emitter-1', uncertainty_m: 120 });

    const [ring] = feature.geometry.coordinates;
    expect(ring[0]).toEqual(ring[ring.length - 1]); // closed ring
    for (const [lon, lat] of ring) {
      const d = distanceMeters(2.5, 1.5, lon, lat);
      expect(d).toBeGreaterThan(120 * 0.98);
      expect(d).toBeLessThan(120 * 1.02);
    }
  });

  it('yields an empty FeatureCollection when no marker has a radius', () => {
    const collection = emitterUncertaintyCirclesGeoJSON([
      { id: 'emitter-2', name: 'B', lon: 0, lat: 0, observed_at: '2026-07-04T00:00:00Z' },
    ]);
    expect(collection).toEqual({ type: 'FeatureCollection', features: [] });
  });
});

function distanceMeters(lon1: number, lat1: number, lon2: number, lat2: number): number {
  // Equirectangular approximation — fine at the small (sub-km) scale zones
  // operate at, matching the same approximation `zoneToCircleFeature` uses.
  const R = 6_371_000;
  const x = ((lon2 - lon1) * Math.PI) / 180 * Math.cos(((lat1 + lat2) / 2 * Math.PI) / 180);
  const y = ((lat2 - lat1) * Math.PI) / 180;
  return Math.sqrt(x * x + y * y) * R;
}

const ZONE: Zone = {
  id: 'zone-1',
  name: 'Home',
  lon: 2.5,
  lat: 1.5,
  radius_m: 200,
  notes: null,
  created_at: '2026-01-01T00:00:00Z',
};

test('zoneToCircleFeature produces a closed polygon ring', () => {
  const feature = zoneToCircleFeature(ZONE);

  expect(feature.type).toBe('Feature');
  expect(feature.geometry.type).toBe('Polygon');
  const [ring] = feature.geometry.coordinates;
  expect(ring.length).toBeGreaterThan(3);
  expect(ring[0]).toEqual(ring[ring.length - 1]);
  expect(feature.properties).toEqual({ id: 'zone-1', name: 'Home', radius_m: 200 });
});

test('zoneToCircleFeature ring points sit ~radius_m from the zone center', () => {
  const feature = zoneToCircleFeature(ZONE);
  const [ring] = feature.geometry.coordinates;

  // Check several points around the ring (not just one) so both the
  // longitude (cos(lat)-scaled) and latitude axes are exercised.
  const samplesToCheck = [0, Math.floor(ring.length / 4), Math.floor(ring.length / 2), Math.floor((3 * ring.length) / 4)];
  for (const i of samplesToCheck) {
    const [lon, lat] = ring[i];
    const d = distanceMeters(ZONE.lon, ZONE.lat, lon, lat);
    expect(d).toBeGreaterThan(ZONE.radius_m * 0.99);
    expect(d).toBeLessThan(ZONE.radius_m * 1.01);
  }
});

test('zoneToCircleFeature at a high latitude still keeps ring points ~radius_m out (cos(lat) longitude scaling)', () => {
  const highLatZone: Zone = { ...ZONE, lat: 60, radius_m: 500 };
  const feature = zoneToCircleFeature(highLatZone);
  const [ring] = feature.geometry.coordinates;

  for (const [lon, lat] of ring) {
    const d = distanceMeters(highLatZone.lon, highLatZone.lat, lon, lat);
    expect(d).toBeGreaterThan(highLatZone.radius_m * 0.98);
    expect(d).toBeLessThan(highLatZone.radius_m * 1.02);
  }
});

test('zonesToCircleGeoJSON wraps one closed polygon feature per zone', () => {
  const zone2: Zone = { ...ZONE, id: 'zone-2', name: 'Office', lon: 3.5, lat: 4.5, radius_m: 100 };
  const collection = zonesToCircleGeoJSON([ZONE, zone2]);

  expect(collection.type).toBe('FeatureCollection');
  expect(collection.features).toHaveLength(2);
  expect(collection.features.map((f) => f.properties.name)).toEqual(['Home', 'Office']);
});

describe('sightingPointsToGeoJSON', () => {
  it('maps points to features carrying the rssi property', () => {
    const fc = sightingPointsToGeoJSON([
      { lon: -84.5, lat: 37.7, signal_strength: -70 },
      { lon: -84.4, lat: 37.6, signal_strength: null },
    ]);
    expect(fc.type).toBe('FeatureCollection');
    expect(fc.features).toHaveLength(2);
    expect(fc.features[0].geometry.coordinates).toEqual([-84.5, 37.7]);
    expect(fc.features[0].properties.rssi).toBe(-70);
    expect(fc.features[1].properties.rssi).toBeNull();
  });
});

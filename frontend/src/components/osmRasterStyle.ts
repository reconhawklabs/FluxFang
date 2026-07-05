// Shared keyless-OSM-raster MapLibre style, used by every map surface in
// this app (`pages/MapView.tsx`'s overview map, `components/EmissionsHeatmap.tsx`'s
// compact per-emitter/per-entity heatmap) — split out so the two don't drift
// from each other.
//
// One raster source hitting OSM's standard tile endpoint, one layer drawing
// it — not a vector style from a hosted (API-keyed) provider like MapTiler/
// Mapbox, since this app has no map-tile API key configured anywhere. Still
// needs *runtime internet access* for the browser to fetch OSM tile images;
// the data layers built on top of this style render regardless of whether
// tiles load, they just draw on a blank background if OSM is unreachable.
import type { StyleSpecification } from 'maplibre-gl';

export const OSM_RASTER_STYLE: StyleSpecification = {
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

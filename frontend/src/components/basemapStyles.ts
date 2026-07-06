// Phase 6 (Map page redesign addendum): the overview map's basemap-style
// switcher (`pages/MapView.tsx`) — three keyless raster options the user can
// switch between without losing the data layers (heatmaps/markers/zones).
//
// Approach (documented per the task brief): rather than calling MapLibre's
// `map.setStyle(...)` — which replaces the *entire* style and drops every
// source/layer this page has added, requiring them all to be re-added on the
// next `'styledata'` event — `MapView` keeps ONE style for its whole
// lifetime, with a single dedicated raster source/layer for "the basemap"
// (`BASEMAP_SOURCE_ID`/`BASEMAP_LAYER_ID`). Switching basemaps just calls
// `map.getSource(BASEMAP_SOURCE_ID).setTiles(newTiles)` (a MapLibre GL JS
// v5 `RasterTileSource` method) to swap the tile template in place — every
// other source/layer (emissions heatmaps, entity/emitter markers, zones) is
// completely untouched. The attribution string isn't something MapLibre's
// `setTiles` updates, so `MapView` renders its own small attribution caption
// from `BASEMAP_OPTIONS` next to the switcher instead of relying on
// MapLibre's built-in `AttributionControl` to notice the swap.
//
// All three are keyless (no API key configured anywhere in this app, same
// rationale as `osmRasterStyle.ts`) but every one of them — including
// "Standard" — needs *runtime internet access* for the browser to fetch tile
// images; the data layers render regardless of whether tiles load.
import { OSM_TILE_URL } from './osmRasterStyle';

export type BasemapId = 'standard' | 'satellite' | 'dark';

export interface BasemapOption {
  id: BasemapId;
  label: string;
  tiles: string[];
  tileSize: number;
  attribution: string;
}

/** `{z}/{y}/{x}` — note the swapped `y`/`x` order versus the OSM/CARTO
 * templates below; ArcGIS Online's tile scheme really does take them in
 * that order. */
const ESRI_WORLD_IMAGERY_URL =
  'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}';

const CARTO_DARK_URL = 'https://basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png';

export const BASEMAP_OPTIONS: BasemapOption[] = [
  {
    id: 'standard',
    label: 'Standard',
    tiles: [OSM_TILE_URL],
    tileSize: 256,
    attribution: '© OpenStreetMap contributors',
  },
  {
    id: 'satellite',
    label: 'Satellite',
    tiles: [ESRI_WORLD_IMAGERY_URL],
    tileSize: 256,
    attribution: 'Esri, Maxar, Earthstar Geographics',
  },
  {
    id: 'dark',
    label: 'Dark',
    tiles: [CARTO_DARK_URL],
    tileSize: 256,
    attribution: '© OpenStreetMap contributors © CARTO',
  },
];

export const DEFAULT_BASEMAP_ID: BasemapId = 'standard';

export function basemapOption(id: BasemapId): BasemapOption {
  return BASEMAP_OPTIONS.find((option) => option.id === id) ?? BASEMAP_OPTIONS[0];
}

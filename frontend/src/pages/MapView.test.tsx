// Task 9.7 acceptance test, restructured for Phase 6 (Map page control
// redesign — grouped checkbox controls, datetime pickers, basemap
// switcher). Per the task brief, GL rendering itself isn't under test here
// (that's what `components/mapData.test.ts` covers) — this file only checks
// the page's non-GL surface: it renders the "Emissions"/"Layers"/"Sources"
// control groups and the basemap switcher without crashing, and asserts the
// all-vs-specific disabled-state wiring + query params those groups drive.
// `maplibre-gl` is mocked wholesale so `new maplibregl.Map(...)` never
// touches a real WebGL canvas (jsdom has none) — see `MapView.tsx`'s module
// doc comment.
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import maplibregl from 'maplibre-gl';
import MapView from './MapView';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Emission, EmissionsPage } from '../api/emissions';
import type { Entity, EntityDetail } from '../api/entities';
import type { Zone } from '../api/zones';
import type { DataSource } from '../api/dataSources';
import type { Emitter } from '../api/emitters';
import type { GpsStatus } from '../api/gps';

// `jumpTo` backs the once-on-load auto-center (Phase 5); `flyTo` backs the
// "recenter to me" button. Both are spies (not just no-ops) so tests below
// can assert on the `{center: [lon, lat]}` they were called with.
//
// `getSource` is source-id-aware (a `Map<string, {setData, setTiles}>`), not
// a fresh mock per call — Phase 6's basemap-switcher test needs to grab the
// SAME `setTiles` spy that `MapView`'s effect called, so a real per-id
// registry (rather than a brand-new `vi.fn()` on every `getSource(...)`
// call) is required.
vi.mock('maplibre-gl', () => {
  class FakeSource {
    setData = vi.fn();
    setTiles = vi.fn();
  }

  class FakeMap {
    // Tracks every constructed instance so tests can grab "the map MapView
    // just created" (`latestFakeMap()` below) without MapView itself
    // exposing its internal `mapRef`.
    static instances: FakeMap[] = [];
    private handlers = new Map<string, () => void>();
    private sources = new Map<string, FakeSource>();
    jumpTo = vi.fn();
    flyTo = vi.fn();
    constructor(_options: unknown) {
      FakeMap.instances.push(this);
    }
    addControl(): void {}
    on(event: string, cb: () => void): void {
      this.handlers.set(event, cb);
      if (event === 'load') cb();
    }
    remove(): void {}
    resize(): void {}
    addSource(id: string): void {
      if (!this.sources.has(id)) this.sources.set(id, new FakeSource());
    }
    addLayer(): void {}
    getSource(id: string) {
      if (!this.sources.has(id)) this.sources.set(id, new FakeSource());
      return this.sources.get(id);
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
    setPaintProperty(): void {}
  }

  class FakeNavigationControl {}

  return { default: { Map: FakeMap, NavigationControl: FakeNavigationControl } };
});

/** The most recently constructed `FakeMap` (there's exactly one per
 * `render(<MapView />)`) — used to assert on `jumpTo`/`flyTo`/source calls
 * without MapView exposing its internal `mapRef`. */
function latestFakeMap() {
  const MapCtor = maplibregl.Map as unknown as {
    instances: Array<{
      jumpTo: ReturnType<typeof vi.fn>;
      flyTo: ReturnType<typeof vi.fn>;
      getSource: (id: string) => { setData: ReturnType<typeof vi.fn>; setTiles: ReturnType<typeof vi.fn> };
    }>;
  };
  return MapCtor.instances[MapCtor.instances.length - 1];
}

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

function mockRoutes(handlers: Record<string, (url: URL, init?: RequestInit) => unknown>) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const raw = typeof input === 'string' ? input : input.toString();
    const url = new URL(raw, 'http://localhost');
    const method = (init?.method ?? 'GET').toUpperCase();
    const key = `${method} ${url.pathname}`;
    const handler = handlers[key];
    if (!handler) {
      return Promise.reject(new Error(`mockRoutes: no route registered for ${key}`));
    }
    return Promise.resolve(jsonResponse(handler(url, init)));
  });
}

const DATA_SOURCE_1: DataSource = {
  id: 'ds-1',
  created_at: '2026-01-01T00:00:00Z',
  kind: 'wifi',
  mode: 'monitor',
  interface: 'wlan0',
  status: 'running',
  config: {},
  last_error: null,
};

const EMISSION_1: Emission = {
  id: 'em-1',
  data_source_id: 'ds-1',
  emitter_id: null,
  session_id: null,
  observed_at: '2026-07-01T00:00:00Z',
  signal_strength: -40,
  lon: 2.5,
  lat: 1.5,
  kind: 'wifi',
  payload: {},
};

const EMISSION_NO_LOCATION: Emission = { ...EMISSION_1, id: 'em-2', lon: null, lat: null };

const EMISSIONS_PAGE: EmissionsPage = { items: [EMISSION_1, EMISSION_NO_LOCATION], total: 2 };

const ENTITY_1: Entity = { id: 'entity-1', name: 'Bob', notes: null, created_at: '2026-06-01T00:00:00Z' };

const ENTITY_1_DETAIL: EntityDetail = {
  ...ENTITY_1,
  last_seen: '2026-07-04T12:00:00Z',
  emitters: [],
  recent_detections: [{ emitter_id: null, lat: 1.5, lon: 2.5, observed_at: '2026-07-04T12:00:00Z' }],
};

const ZONE_1: Zone = {
  id: 'zone-1',
  name: 'Home',
  lon: 2.5,
  lat: 1.5,
  radius_m: 50,
  notes: null,
  created_at: '2026-01-01T00:00:00Z',
};

/** An auto-classified WiFi client emitter — used to give `categories` (the
 * overview map's category-layer derivation, Task C) a `"wifi"` entry so the
 * "All WiFi" toggle test below has something to find. Most existing tests
 * in this file don't care about emitters at all (`baseRoutes` defaults
 * `GET /api/emitters` to an empty list), so no category toggles render for
 * them. */
const EMITTER_WIFI: Emitter = {
  id: 'emitter-1',
  name: 'WiFi Client aa:bb:cc:dd:ee:ff',
  type: null,
  emitter_type: 'wifi_client',
  attributes: { src_mac: 'aa:bb:cc:dd:ee:ff' },
  match_enabled: true,
  type_label: 'WiFi Client',
  category: 'wifi',
  entity_id: null,
  match_criteria: { match: 'all', conditions: [] },
  first_seen_at: '2026-07-05T00:00:00Z',
  last_seen_at: '2026-07-05T01:00:00Z',
  created_at: '2026-07-05T00:00:00Z',
};

/** No running gps source / no fix — the default for tests that don't care
 * about the recenter/auto-center behavior, so the "recenter to me" button
 * renders disabled and no `jumpTo` fires. */
const GPS_STATUS_NO_FIX: GpsStatus = {
  source_running: false,
  has_fix: false,
  lat: null,
  lon: null,
  quality: null,
  fix_age_seconds: null,
  status: 'disabled',
};

const GPS_STATUS_FIX: GpsStatus = {
  source_running: true,
  has_fix: true,
  lat: 1.5,
  lon: 2.5,
  quality: 4,
  fix_age_seconds: 1.2,
  status: 'active',
};

function baseRoutes(overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {}) {
  return {
    'GET /api/data-sources': () => [DATA_SOURCE_1],
    'GET /api/emissions': () => EMISSIONS_PAGE,
    'GET /api/emitters': () => ({ items: [] as Emitter[], total: 0 }),
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
    'GET /api/entities/entity-1': () => ENTITY_1_DETAIL,
    'GET /api/zones': () => [ZONE_1],
    'GET /api/gps/status': () => GPS_STATUS_NO_FIX,
    ...overrides,
  };
}

test('renders the Emissions/Layers/Sources control groups without crashing', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByText('Emissions')).toBeInTheDocument();
  expect(screen.getByText('Layers')).toBeInTheDocument();
  expect(screen.getByText('Sources')).toBeInTheDocument();

  expect(screen.getByLabelText('All Emissions')).toBeInTheDocument();
  expect(screen.getByLabelText('Zones')).toBeInTheDocument();
  expect(screen.getByLabelText('Entities')).toBeInTheDocument();
  expect(screen.getByLabelText('Emitters')).toBeInTheDocument();
  expect(screen.getByLabelText('All Sources')).toBeInTheDocument();
  expect(screen.getByTestId('maplibre-container')).toBeInTheDocument();

  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
});

test('renders one Sources checkbox per GET /api/data-sources entry, replacing the old dropdown', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByLabelText('All Sources')).toBeInTheDocument();
  expect(await screen.findByLabelText('wifi (wlan0)')).toBeInTheDocument();
  expect(screen.queryByLabelText(/data source/i)).not.toBeInTheDocument();
});

test('toggling the Emissions/Layers checkboxes does not crash the page', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const allEmissions = await screen.findByLabelText('All Emissions');
  expect(allEmissions).toBeChecked();
  fireEvent.click(allEmissions);
  expect(allEmissions).not.toBeChecked();

  fireEvent.click(screen.getByLabelText('Entities'));
  fireEvent.click(screen.getByLabelText('Zones'));
  fireEvent.click(screen.getByLabelText('Emitters'));
});

test('a distinct emitter category ("wifi") drives an "All WiFi" toggle, backed by its own emitter_category-filtered query', async () => {
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/emitters': () => ({ items: [EMITTER_WIFI], total: 1 }) }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const wifiToggle = await screen.findByLabelText('All WiFi');
  // "All Emissions" defaults checked, which covers/disables the per-category
  // toggles (see the dedicated disabled-state test below) — it still shows
  // checked to reflect "covered by All Emissions".
  expect(wifiToggle).toBeChecked();
  expect(wifiToggle).toBeDisabled();

  // The category layer's own `GET /api/emissions?...emitter_category=wifi`
  // query fires (fetching data for the layer, regardless of its current
  // visibility toggle state — same convention as the existing "All
  // emissions" heatmap, whose query isn't gated on the toggle either).
  await waitFor(() => {
    const categoryCall = fetchMock.mock.calls.find(([url]) => {
      const parsed = new URL(String(url), 'http://localhost');
      return parsed.pathname === '/api/emissions' && parsed.searchParams.get('emitter_category') === 'wifi';
    });
    expect(categoryCall).toBeDefined();
  });
});

test('an emitter with no category (plain user-made emitter) contributes no category toggle', async () => {
  const plainEmitter: Emitter = { ...EMITTER_WIFI, id: 'emitter-2', emitter_type: null, category: null };
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/emitters': () => ({ items: [plainEmitter], total: 1 }) }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  await screen.findByLabelText('All Emissions');
  expect(screen.queryByLabelText(/^All (?!Emissions|Sources)/)).not.toBeInTheDocument();
});

test('checking "All Emissions" disables the per-category checkboxes; unchecking it re-enables them', async () => {
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/emitters': () => ({ items: [EMITTER_WIFI], total: 1 }) }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const allEmissions = await screen.findByLabelText('All Emissions');
  const wifiToggle = await screen.findByLabelText('All WiFi');

  expect(allEmissions).toBeChecked();
  expect(wifiToggle).toBeDisabled();
  expect(wifiToggle).toBeChecked();

  fireEvent.click(allEmissions);

  expect(allEmissions).not.toBeChecked();
  expect(wifiToggle).not.toBeDisabled();
});

test('the Layers group toggles Zones/Entities/Emitters independently', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const zones = await screen.findByLabelText('Zones');
  const entities = screen.getByLabelText('Entities');
  const emitters = screen.getByLabelText('Emitters');

  expect(zones).toBeChecked();
  expect(entities).toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(zones);
  expect(zones).not.toBeChecked();
  expect(entities).toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(entities);
  expect(zones).not.toBeChecked();
  expect(entities).not.toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(emitters);
  expect(zones).not.toBeChecked();
  expect(entities).not.toBeChecked();
  expect(emitters).not.toBeChecked();
});

test('"All Sources" disables the per-source checkboxes; unchecking it and selecting one source adds data_source_id to the emissions query', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const allSources = await screen.findByLabelText('All Sources');
  const sourceCheckbox = await screen.findByLabelText('wifi (wlan0)');

  expect(allSources).toBeChecked();
  expect(sourceCheckbox).toBeDisabled();
  expect(sourceCheckbox).not.toBeChecked();

  fireEvent.click(allSources);
  expect(allSources).not.toBeChecked();
  expect(sourceCheckbox).not.toBeDisabled();

  fireEvent.click(sourceCheckbox);
  expect(sourceCheckbox).toBeChecked();

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([url]) => {
      const parsed = new URL(String(url), 'http://localhost');
      return parsed.pathname === '/api/emissions' && parsed.searchParams.get('data_source_id') === 'ds-1';
    });
    expect(call).toBeDefined();
  });
});

test('the basemap switcher offers Standard/Satellite/Dark, defaulting to Standard, and switching to Satellite swaps the map tiles', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const select = await screen.findByLabelText(/basemap/i);
  expect((select as HTMLSelectElement).value).toBe('standard');
  expect(within(select).getAllByRole('option').map((option) => option.textContent)).toEqual([
    'Standard',
    'Satellite',
    'Dark',
  ]);

  fireEvent.change(select, { target: { value: 'satellite' } });
  expect((select as HTMLSelectElement).value).toBe('satellite');

  await waitFor(() => {
    expect(latestFakeMap().getSource('basemap-source').setTiles).toHaveBeenCalledWith(
      expect.arrayContaining([expect.stringContaining('arcgisonline.com')]),
    );
  });
});

test('with no GPS fix, the "Recenter to me" button is disabled and the map never auto-centers', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole('button', { name: /recenter/i });
  await waitFor(() => expect(button).toBeDisabled());
  expect(latestFakeMap().jumpTo).not.toHaveBeenCalled();
});

test('with a GPS fix, the map auto-centers on load and the "Recenter to me" button is enabled', async () => {
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/gps/status': () => GPS_STATUS_FIX }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole('button', { name: /recenter/i });
  await waitFor(() => expect(button).toBeEnabled());
  await waitFor(() =>
    expect(latestFakeMap().jumpTo).toHaveBeenCalledWith(
      expect.objectContaining({ center: [GPS_STATUS_FIX.lon, GPS_STATUS_FIX.lat] }),
    ),
  );
});

test('clicking "Recenter to me" flies the map to the current GPS fix', async () => {
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/gps/status': () => GPS_STATUS_FIX }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole('button', { name: /recenter/i });
  await waitFor(() => expect(button).toBeEnabled());

  fireEvent.click(button);

  expect(latestFakeMap().flyTo).toHaveBeenCalledWith(
    expect.objectContaining({ center: [GPS_STATUS_FIX.lon, GPS_STATUS_FIX.lat] }),
  );
});

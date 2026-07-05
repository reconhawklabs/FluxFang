// Task 9.7 acceptance test. Per the task brief, GL rendering itself isn't
// under test here (that's what `components/mapData.test.ts` covers) — this
// file only checks the page's non-GL surface: it renders the layer-toggle
// controls (Heatmap/Entities/Zones) and the filter row without crashing.
// `maplibre-gl` is mocked wholesale so `new maplibregl.Map(...)` never
// touches a real WebGL canvas (jsdom has none) — see `MapView.tsx`'s module
// doc comment.
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import MapView from './MapView';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Emission, EmissionsPage } from '../api/emissions';
import type { Entity, EntityDetail } from '../api/entities';
import type { Zone } from '../api/zones';
import type { DataSource } from '../api/dataSources';
import type { Emitter } from '../api/emitters';

vi.mock('maplibre-gl', () => {
  class FakeMap {
    private handlers = new Map<string, () => void>();
    constructor(_options: unknown) {}
    addControl(): void {}
    on(event: string, cb: () => void): void {
      this.handlers.set(event, cb);
      if (event === 'load') cb();
    }
    remove(): void {}
    addSource(): void {}
    addLayer(): void {}
    getSource() {
      return { setData: vi.fn() };
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
  }

  class FakeNavigationControl {}

  return { default: { Map: FakeMap, NavigationControl: FakeNavigationControl } };
});

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

function baseRoutes(overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {}) {
  return {
    'GET /api/data-sources': () => [DATA_SOURCE_1],
    'GET /api/emissions': () => EMISSIONS_PAGE,
    'GET /api/emitters': () => [] as Emitter[],
    'GET /api/entities': () => [ENTITY_1],
    'GET /api/entities/entity-1': () => ENTITY_1_DETAIL,
    'GET /api/zones': () => [ZONE_1],
    ...overrides,
  };
}

test('renders the All emissions/Entities/Zones layer toggles without crashing', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByLabelText('All emissions')).toBeInTheDocument();
  expect(screen.getByLabelText('Entities')).toBeInTheDocument();
  expect(screen.getByLabelText('Zones')).toBeInTheDocument();
  expect(screen.getByTestId('maplibre-container')).toBeInTheDocument();

  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
});

test('renders the data-source filter populated from GET /api/data-sources', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const select = await screen.findByLabelText(/data source/i);
  await waitFor(() => expect(within(select).queryAllByRole('option')).toHaveLength(2));
});

test('toggling a layer checkbox does not crash the page', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const heatmapToggle = await screen.findByLabelText('All emissions');
  expect(heatmapToggle).toBeChecked();
  fireEvent.click(heatmapToggle);
  expect(heatmapToggle).not.toBeChecked();

  fireEvent.click(screen.getByLabelText('Entities'));
  fireEvent.click(screen.getByLabelText('Zones'));
});

test('a distinct emitter category ("wifi") drives an "All WiFi" toggle, backed by its own emitter_category-filtered query', async () => {
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/emitters': () => [EMITTER_WIFI] }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  const wifiToggle = await screen.findByLabelText('All WiFi');
  expect(wifiToggle).toBeChecked();

  // The category layer's own `GET /api/emissions?...emitter_category=wifi`
  // query fires (fetching data for the layer, regardless of its current
  // visibility toggle state — same convention as the existing "All
  // emissions" heatmap, whose query isn't gated on `visibility.heatmap`
  // either).
  await waitFor(() => {
    const categoryCall = fetchMock.mock.calls.find(([url]) => {
      const parsed = new URL(String(url), 'http://localhost');
      return parsed.pathname === '/api/emissions' && parsed.searchParams.get('emitter_category') === 'wifi';
    });
    expect(categoryCall).toBeDefined();
  });

  // Toggling it off flips its checked state without crashing the page —
  // the layer's visibility, not its data fetch, is what the toggle drives.
  fireEvent.click(wifiToggle);
  expect(wifiToggle).not.toBeChecked();
});

test('an emitter with no category (plain user-made emitter) contributes no category toggle', async () => {
  const plainEmitter: Emitter = { ...EMITTER_WIFI, id: 'emitter-2', emitter_type: null, category: null };
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/emitters': () => [plainEmitter] }));
  vi.stubGlobal('fetch', fetchMock);

  render(<MapView />, { wrapper });

  await screen.findByLabelText('All emissions');
  expect(screen.queryByLabelText(/^All (?!emissions)/)).not.toBeInTheDocument();
});

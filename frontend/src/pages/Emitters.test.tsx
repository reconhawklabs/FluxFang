import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Emitters from './Emitters';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Emitter } from '../api/emitters';
import type { Entity } from '../api/entities';
import type { Emission } from '../api/emissions';

// `EmitterDetail` embeds `EmissionsHeatmap` (Task C), which inits a real
// MapLibre map whenever it's given non-empty points — mocked wholesale here
// (same convention as `MapView.test.tsx`) so that never touches a real
// WebGL canvas jsdom doesn't have.
vi.mock('maplibre-gl', () => {
  class FakeMap {
    constructor(_options: unknown) {}
    addControl(): void {}
    on(event: string, cb: () => void): void {
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
    fitBounds(): void {}
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

/** Method+pathname-aware fetch mock (same convention as
 * `DataSources.test.tsx`'s `mockMethodRoutes` / `Emissions.test.tsx`'s
 * `mockRoutes`) — this page hits `GET /api/emitters`, `GET /api/entities`,
 * `POST /api/entities`, and `PATCH /api/emitters/:id`, so routing needs
 * both the method and the (sometimes dynamic-id) pathname. */
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

const EMITTER_UNASSIGNED: Emitter = {
  id: 'emitter-1',
  name: 'Unknown AP',
  type: 'wifi-ap',
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: null,
  category: null,
  entity_id: null,
  match_criteria: { match: 'all', conditions: [{ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }] },
  first_seen_at: '2026-07-01T00:00:00Z',
  last_seen_at: '2026-07-04T12:00:00Z',
  created_at: '2026-07-01T00:00:00Z',
};

const EMITTER_ASSIGNED: Emitter = {
  id: 'emitter-2',
  name: "Neighbor's Router",
  type: 'wifi-ap',
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: null,
  category: null,
  entity_id: 'entity-1',
  match_criteria: { match: 'all', conditions: [] },
  first_seen_at: null,
  last_seen_at: null,
  created_at: '2026-07-01T00:00:00Z',
};

/** An auto-classified WiFi client emitter (Phase A backend / Phase B
 * frontend, emitter auto-classification design doc) — has a randomized
 * source MAC flagged, and its rule is currently enabled. */
const EMITTER_CLIENT: Emitter = {
  id: 'emitter-3',
  name: 'WiFi Client aa:bb:cc:dd:ee:ff',
  type: null,
  emitter_type: 'wifi_client',
  attributes: { src_mac: 'aa:bb:cc:dd:ee:ff', randomized_mac: true },
  match_enabled: true,
  type_label: 'WiFi Client',
  category: 'wifi',
  entity_id: null,
  match_criteria: { match: 'all', conditions: [{ field: 'src_mac', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }] },
  first_seen_at: '2026-07-05T00:00:00Z',
  last_seen_at: '2026-07-05T01:00:00Z',
  created_at: '2026-07-05T00:00:00Z',
};

/** An auto-classified WiFi access-point emitter with a visible SSID. */
const EMITTER_AP: Emitter = {
  id: 'emitter-4',
  name: 'WiFi AP "CoffeeShop" (11:22:33:44:55:66)',
  type: null,
  emitter_type: 'wifi_access_point',
  attributes: { ssid: 'CoffeeShop', bssid: '11:22:33:44:55:66' },
  match_enabled: false,
  type_label: 'WiFi Access Point',
  category: 'wifi',
  entity_id: null,
  match_criteria: { match: 'all', conditions: [{ field: 'bssid', op: 'eq', value: '11:22:33:44:55:66' }] },
  first_seen_at: '2026-07-05T00:00:00Z',
  last_seen_at: '2026-07-05T01:00:00Z',
  created_at: '2026-07-05T00:00:00Z',
};

const ENTITY_1: Entity = {
  id: 'entity-1',
  name: 'Bob',
  notes: null,
  created_at: '2026-06-01T00:00:00Z',
};

test('renders emitter rows with name/type/last-seen and the associated entity name', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_UNASSIGNED, EMITTER_ASSIGNED], total: 2 }),
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-1')).toBeInTheDocument());

  const row1 = screen.getByTestId('emitter-row-emitter-1');
  expect(within(row1).getByText('Unknown AP')).toBeInTheDocument();
  expect(within(row1).getByText('wifi-ap')).toBeInTheDocument();
  expect(within(row1).getByText(new Date('2026-07-04T12:00:00Z').toLocaleString())).toBeInTheDocument();
  expect(screen.getByTestId('emitter-entity-emitter-1')).toHaveTextContent('—'); // unassigned entity column

  expect(screen.getByTestId('emitter-entity-emitter-2')).toHaveTextContent('Bob'); // resolved from GET /api/entities
});

test('associate-to-existing: selecting an entity PATCHes {entity_id: <selected>}', async () => {
  const patched: Emitter = { ...EMITTER_UNASSIGNED, entity_id: 'entity-1' };
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
    'PATCH /api/emitters/emitter-1': () => patched,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-1')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-1');
  fireEvent.change(within(row).getByLabelText(/associate .* to an entity/i), { target: { value: 'entity-1' } });

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/emitters/emitter-1',
      expect.objectContaining({ method: 'PATCH' }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/emitters/emitter-1' && init?.method === 'PATCH',
  );
  expect(patchCall).toBeDefined();
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ entity_id: 'entity-1' });
});

test('detach: clicking Detach PATCHes {entity_id: null}', async () => {
  const detached: Emitter = { ...EMITTER_ASSIGNED, entity_id: null };
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_ASSIGNED], total: 1 }),
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
    'PATCH /api/emitters/emitter-2': () => detached,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-2')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-2');
  fireEvent.click(within(row).getByRole('button', { name: /detach/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/emitters/emitter-2',
      expect.objectContaining({ method: 'PATCH' }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/emitters/emitter-2' && init?.method === 'PATCH',
  );
  expect(patchCall).toBeDefined();
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ entity_id: null });
});

test('create entity & associate: entering a name creates the entity then PATCHes the emitter, and refetches the emitter list', async () => {
  const NEW_ENTITY: Entity = { id: 'entity-new', name: 'Coffee Shop', notes: null, created_at: '2026-07-05T00:00:00Z' };
  const associated: Emitter = { ...EMITTER_UNASSIGNED, entity_id: 'entity-new' };

  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
    'POST /api/entities': () => NEW_ENTITY,
    'PATCH /api/emitters/emitter-1': () => associated,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-1')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-1');
  const getEmittersCallCountBefore = fetchMock.mock.calls.filter(
    ([url, init]) => String(url) === '/api/emitters?limit=500' && (init?.method ?? 'GET') === 'GET',
  ).length;

  fireEvent.click(within(row).getByRole('button', { name: /new entity/i }));
  fireEvent.change(within(row).getByLabelText(/new entity name/i), { target: { value: 'Coffee Shop' } });
  fireEvent.click(within(row).getByRole('button', { name: /create & associate/i }));

  // Both calls happen: POST /api/entities then PATCH /api/emitters/:id.
  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith('/api/entities', expect.objectContaining({ method: 'POST' })),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/entities' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, postInit] = postCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(postInit.body as string)).toEqual({ name: 'Coffee Shop' });

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/emitters/emitter-1',
      expect.objectContaining({ method: 'PATCH' }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/emitters/emitter-1' && init?.method === 'PATCH',
  );
  expect(patchCall).toBeDefined();
  const [, patchInit] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(patchInit.body as string)).toEqual({ entity_id: 'entity-new' });

  // The emitter list refetches (invalidated on success).
  await waitFor(() => {
    const getEmittersCallCountAfter = fetchMock.mock.calls.filter(
      ([url, init]) => String(url) === '/api/emitters?limit=500' && (init?.method ?? 'GET') === 'GET',
    ).length;
    expect(getEmittersCallCountAfter).toBeGreaterThan(getEmittersCallCountBefore);
  });
});

// --- Phase B: emitter auto-classification display + rule toggle ---
// (emitter auto-classification design doc's Frontend section)

test('a wifi_client emitter renders its type badge, src_mac, and a "Randomized MAC" badge', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-3');
  expect(within(row).getByText('WiFi Client')).toBeInTheDocument();
  expect(within(row).getByText('aa:bb:cc:dd:ee:ff')).toHaveClass('font-mono');
  expect(within(row).getByText(/randomized mac/i)).toBeInTheDocument();
});

test('a wifi_access_point emitter renders its type badge, SSID, and BSSID (monospace)', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_AP], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-4')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-4');
  expect(within(row).getByText('WiFi Access Point')).toBeInTheDocument();
  expect(within(row).getByText('CoffeeShop')).toBeInTheDocument();
  expect(within(row).getByText('11:22:33:44:55:66')).toHaveClass('font-mono');
});

test('an AP emitter with no SSID shows "Hidden"', async () => {
  const hidden: Emitter = { ...EMITTER_AP, attributes: { ssid: '', bssid: '11:22:33:44:55:66' } };
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [hidden], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-4')).toBeInTheDocument());

  expect(within(screen.getByTestId('emitter-row-emitter-4')).getByText('Hidden')).toBeInTheDocument();
});

test('toggling the rule switch PATCHes {match_enabled: false} and shows a disabled helper', async () => {
  const disabled: Emitter = { ...EMITTER_CLIENT, match_enabled: false };
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
    'PATCH /api/emitters/emitter-3': () => disabled,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-3');
  const ruleSwitch = within(row).getByRole('switch', { name: /rule enabled/i });
  expect(ruleSwitch).toBeChecked();

  fireEvent.click(ruleSwitch);

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/emitters/emitter-3',
      expect.objectContaining({ method: 'PATCH' }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/emitters/emitter-3' && init?.method === 'PATCH',
  );
  expect(patchCall).toBeDefined();
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ match_enabled: false });
});

test('a disabled rule shows a helper explaining new matches will not auto-attach', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_AP], total: 1 }), // EMITTER_AP has match_enabled: false
    'GET /api/entities': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-4')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-4');
  expect(within(row).getByText(/won.t auto-attach/i)).toBeInTheDocument();
});

test('manual randomized override: toggling it PATCHes the full attributes object with randomized_mac flipped', async () => {
  const flipped: Emitter = {
    ...EMITTER_CLIENT,
    attributes: { ...EMITTER_CLIENT.attributes, randomized_mac: false },
  };
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
    'PATCH /api/emitters/emitter-3': () => flipped,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-3');
  fireEvent.click(within(row).getByRole('button', { name: /mark as not randomized/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/emitters/emitter-3',
      expect.objectContaining({ method: 'PATCH' }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/emitters/emitter-3' && init?.method === 'PATCH',
  );
  expect(patchCall).toBeDefined();
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({
    attributes: { src_mac: 'aa:bb:cc:dd:ee:ff', randomized_mac: false },
  });
});

test('expanded detail shows the match_criteria rule for an auto-classified emitter', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
    'GET /api/emissions': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  fireEvent.click(screen.getByRole('button', { name: 'WiFi Client aa:bb:cc:dd:ee:ff' }));

  const detail = await screen.findByTestId('emitter-detail-emitter-3');
  expect(within(detail).getByText(/src_mac eq aa:bb:cc:dd:ee:ff/i)).toBeInTheDocument();
});

const LOCATED_EMISSION: Emission = {
  id: 'em-1',
  data_source_id: 'ds-1',
  emitter_id: 'emitter-3',
  session_id: null,
  observed_at: '2026-07-05T00:00:00Z',
  signal_strength: -40,
  lon: 2.5,
  lat: 1.5,
  kind: 'wifi',
  payload: {},
};

const UNLOCATED_EMISSION: Emission = { ...LOCATED_EMISSION, id: 'em-2', lon: null, lat: null };

test('expanded detail renders a detection heatmap fed by located emissions for that emitter', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
    'GET /api/emissions': () => ({ items: [LOCATED_EMISSION, UNLOCATED_EMISSION], total: 2 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  fireEvent.click(screen.getByRole('button', { name: 'WiFi Client aa:bb:cc:dd:ee:ff' }));

  const detail = await screen.findByTestId('emitter-detail-emitter-3');
  expect(within(detail).getByText('Detection heatmap')).toBeInTheDocument();
  expect(await within(detail).findByTestId('emissions-heatmap-container')).toBeInTheDocument();
});

test('expanded detail shows the heatmap empty state when the emitter has no located emissions', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => ({ items: [EMITTER_CLIENT], total: 1 }),
    'GET /api/entities': () => ({ items: [], total: 0 }),
    'GET /api/emissions': () => ({ items: [], total: 0 }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-3')).toBeInTheDocument());

  fireEvent.click(screen.getByRole('button', { name: 'WiFi Client aa:bb:cc:dd:ee:ff' }));

  const detail = await screen.findByTestId('emitter-detail-emitter-3');
  expect(await within(detail).findByText('No located detections yet.')).toBeInTheDocument();
});

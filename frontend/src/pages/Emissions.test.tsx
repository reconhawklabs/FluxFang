import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Emissions from './Emissions';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Emission } from '../api/emissions';
import type { Emitter } from '../api/emitters';
import type { FieldDef } from '../types/catalog';

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

/** Method+pathname-aware fetch mock (same convention as
 * `DataSources.test.tsx`'s `mockMethodRoutes`) — this page hits several
 * distinct GET paths (`/api/emissions`, `/api/emitters`, `/api/catalog/wifi`)
 * plus a POST to `/api/emitters`, so routing needs both dimensions. `handler`
 * receives the parsed `URL` so a test can assert on query params or vary the
 * response by them. */
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

const WIFI_CATALOG: FieldDef[] = [
  {
    key: 'bssid',
    label: 'BSSID',
    type: 'mac',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'matches', label: 'contains / matches' },
    ],
  },
  {
    key: 'ssid',
    label: 'SSID',
    type: 'text',
    ops: [{ code: 'eq', label: 'is exactly' }],
  },
  {
    key: 'channel',
    label: 'Channel',
    type: 'number',
    ops: [{ code: 'gte', label: 'is at least' }],
  },
];

const EMISSION_1: Emission = {
  id: 'e1',
  data_source_id: 'ds1',
  emitter_id: null,
  session_id: null,
  observed_at: '2026-07-05T12:00:00Z',
  signal_strength: -55,
  lon: -122.4,
  lat: 37.7,
  kind: 'wifi',
  payload: { bssid: 'aa:bb:cc:dd:ee:ff', ssid: 'CoffeeShop', channel: 6 },
};

const EMISSION_2: Emission = {
  id: 'e2',
  data_source_id: 'ds1',
  emitter_id: 'emitter-1',
  session_id: null,
  observed_at: '2026-07-05T12:05:00Z',
  signal_strength: -70,
  lon: null,
  lat: null,
  kind: 'wifi',
  payload: { bssid: '11:22:33:44:55:66', ssid: 'Home', channel: 11 },
};

const EMITTER_1: Emitter = {
  id: 'emitter-1',
  name: 'My Router',
  type: null,
  entity_id: null,
  match_criteria: {},
  first_seen_at: null,
  last_seen_at: null,
  created_at: '2026-07-01T00:00:00Z',
};

test('renders emission rows (bssid/channel/rssi) and the total from a mocked response', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emissions': () => ({ items: [EMISSION_1, EMISSION_2], total: 2 }),
    'GET /api/emitters': () => [EMITTER_1],
    'GET /api/catalog/wifi': () => WIFI_CATALOG,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emissions />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('emission-row-e1')).toBeInTheDocument());

  const row1 = screen.getByTestId('emission-row-e1');
  expect(within(row1).getByText('aa:bb:cc:dd:ee:ff')).toHaveClass('font-mono');
  expect(within(row1).getByText('6')).toBeInTheDocument();
  expect(within(row1).getByText('-55')).toBeInTheDocument();
  expect(within(row1).getByText('—')).toBeInTheDocument(); // unassigned emitter column

  const row2 = screen.getByTestId('emission-row-e2');
  expect(within(row2).getByText('My Router')).toBeInTheDocument();

  expect(screen.getByTestId('emissions-total')).toHaveTextContent('2 emissions');
});

test('selecting an emission and assigning prefills RuleBuilder with bssid eq <value> and POSTs match_criteria', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emissions': () => ({ items: [EMISSION_1, EMISSION_2], total: 2 }),
    'GET /api/emitters': () => [EMITTER_1],
    'GET /api/catalog/wifi': () => WIFI_CATALOG,
    'GET /api/emitters/preview': () => ({ match_count: 4 }),
    'POST /api/emitters': () => ({
      emitter: { ...EMITTER_1, id: 'emitter-2', name: 'Coffee Shop AP' },
      attached_count: 4,
    }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emission-row-e1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText('Select emission e1'));
  fireEvent.click(screen.getByRole('button', { name: /assign to emitter/i }));

  // RuleBuilder's field catalog fetch resolves asynchronously; "BSSID"
  // also appears as a table column header, so scope the wait to the modal
  // dialog via its heading instead of that ambiguous text.
  await screen.findByRole('heading', { name: /assign 1 emission to emitter/i });
  await waitFor(() => expect(screen.getByLabelText(/field/i)).toHaveValue('bssid'));

  expect(screen.getByLabelText(/field/i)).toHaveValue('bssid');
  expect(screen.getByLabelText(/operator/i)).toHaveValue('eq');
  expect(screen.getByLabelText(/value/i)).toHaveValue('aa:bb:cc:dd:ee:ff');

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Coffee Shop AP' } });
  fireEvent.click(screen.getByRole('button', { name: /^assign$/i }));

  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(/assigned 4 emission/i));

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/emitters');
  const body = JSON.parse(init.body as string);
  expect(body.name).toBe('Coffee Shop AP');
  expect(body.match_criteria).toEqual({
    match: 'all',
    conditions: [{ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' }],
  });
});

test('paging (Next) clears the row selection so "Assign to emitter" can never no-op on a stale id', async () => {
  // `total: 60` with `DEFAULT_LIMIT` (50) makes the Next button eligible
  // (offset + limit < total); the handler varies `items` by the `offset`
  // query param so paging genuinely swaps the rendered rows, matching how
  // the real API behaves.
  const fetchMock = mockRoutes({
    'GET /api/emissions': (url) =>
      url.searchParams.get('offset') === '50'
        ? { items: [EMISSION_2], total: 60 }
        : { items: [EMISSION_1], total: 60 },
    'GET /api/emitters': () => [EMITTER_1],
    'GET /api/catalog/wifi': () => WIFI_CATALOG,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emission-row-e1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText('Select emission e1'));
  expect(screen.getByRole('button', { name: /assign to emitter \(1\)/i })).toBeEnabled();

  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  await waitFor(() => expect(screen.getByTestId('emission-row-e2')).toBeInTheDocument());

  const assignButton = screen.getByRole('button', { name: /assign to emitter \(0\)/i });
  expect(assignButton).toBeDisabled();

  // Clicking it (were it somehow enabled) must not silently no-op: the
  // modal's render guard is `showAssignModal && seedEmission`, so with no
  // selection there's no seed and no dialog — assert that stays true.
  fireEvent.click(assignButton);
  expect(screen.queryByRole('heading', { name: /assign .* to emitter/i })).not.toBeInTheDocument();
});

test('a filter change (unassigned-only) refetches emissions with the matching query params', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emissions': () => ({ items: [EMISSION_1, EMISSION_2], total: 2 }),
    'GET /api/emitters': () => [EMITTER_1],
    'GET /api/catalog/wifi': () => WIFI_CATALOG,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emission-row-e1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText(/unassigned only/i));

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), 'http://localhost');
      return url.pathname === '/api/emissions' && url.searchParams.get('unassigned') === 'true';
    });
    expect(call).toBeDefined();
  });
});

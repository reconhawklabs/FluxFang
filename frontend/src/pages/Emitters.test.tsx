import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Emitters from './Emitters';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Emitter } from '../api/emitters';
import type { Entity } from '../api/entities';

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
  entity_id: 'entity-1',
  match_criteria: { match: 'all', conditions: [] },
  first_seen_at: null,
  last_seen_at: null,
  created_at: '2026-07-01T00:00:00Z',
};

const ENTITY_1: Entity = {
  id: 'entity-1',
  name: 'Bob',
  notes: null,
  created_at: '2026-06-01T00:00:00Z',
};

test('renders emitter rows with name/type/last-seen and the associated entity name', async () => {
  const fetchMock = mockRoutes({
    'GET /api/emitters': () => [EMITTER_UNASSIGNED, EMITTER_ASSIGNED],
    'GET /api/entities': () => [ENTITY_1],
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
    'GET /api/emitters': () => [EMITTER_UNASSIGNED],
    'GET /api/entities': () => [ENTITY_1],
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
    'GET /api/emitters': () => [EMITTER_ASSIGNED],
    'GET /api/entities': () => [ENTITY_1],
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
    'GET /api/emitters': () => [EMITTER_UNASSIGNED],
    'GET /api/entities': () => [ENTITY_1],
    'POST /api/entities': () => NEW_ENTITY,
    'PATCH /api/emitters/emitter-1': () => associated,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('emitter-row-emitter-1')).toBeInTheDocument());

  const row = screen.getByTestId('emitter-row-emitter-1');
  const getEmittersCallCountBefore = fetchMock.mock.calls.filter(
    ([url, init]) => String(url) === '/api/emitters' && (init?.method ?? 'GET') === 'GET',
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
      ([url, init]) => String(url) === '/api/emitters' && (init?.method ?? 'GET') === 'GET',
    ).length;
    expect(getEmittersCallCountAfter).toBeGreaterThan(getEmittersCallCountBefore);
  });
});

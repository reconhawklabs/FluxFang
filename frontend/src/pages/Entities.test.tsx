// Task 9.6 acceptance tests. The key RED->GREEN target per the task brief:
// choosing "When enters zone" in the "Add Alert" form reveals a zone
// dropdown (populated from `GET /api/zones`), and submitting posts
// `/api/alert-rules` with `on: "enters_zone"`, the chosen `zone_id`,
// `target_type: "entity"`/`target_id: <entity.id>`, and
// `method_ids: [<chosen>]`. Also covers: "When detected" doesn't
// require/show a zone, and the expanded detail view surfaces the entity's
// emitters + aggregate `last_seen`.
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Entities from './Entities';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Entity, EntityDetail } from '../api/entities';
import type { Zone } from '../api/zones';
import type { AlertMethod } from '../api/alertMethods';
import type { AlertRule } from '../api/alertRules';

// `EntityDetail` embeds `EmissionsHeatmap` (Task C), which inits a real
// MapLibre map whenever it's given non-empty points (this file's own
// `ENTITY_1_DETAIL.recent_detections` fixture is located) — mocked
// wholesale here (same convention as `MapView.test.tsx`) so that never
// touches a real WebGL canvas jsdom doesn't have.
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

/** Method+pathname-aware fetch mock — same convention as
 * `DataSources.test.tsx`'s `mockMethodRoutes` / `Emitters.test.tsx`'s
 * `mockRoutes`. This page hits `GET/POST /api/entities[/:id]`,
 * `GET /api/zones`, `GET /api/alert-methods`, `GET/POST /api/alert-rules`,
 * and (when the content-match toggle is used) `GET /api/catalog/:kind`. */
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

const ENTITY_1: Entity = {
  id: 'entity-1',
  name: 'Bob',
  notes: 'Neighbor',
  created_at: '2026-06-01T00:00:00Z',
};

const ENTITY_1_DETAIL: EntityDetail = {
  ...ENTITY_1,
  last_seen: '2026-07-04T12:00:00Z',
  emitters: [
    {
      id: 'emitter-1',
      name: "Bob's Phone",
      type: 'wifi-client',
      entity_id: 'entity-1',
      match_criteria: { match: 'all', conditions: [] },
      first_seen_at: '2026-06-01T00:00:00Z',
      last_seen_at: '2026-07-04T12:00:00Z',
      created_at: '2026-06-01T00:00:00Z',
      emitter_type: null,
      attributes: {},
      match_enabled: true,
      type_label: 'wifi-client',
      category: null,
    },
  ],
  recent_detections: [{ emitter_id: 'emitter-1', lat: 1.5, lon: 2.5, observed_at: '2026-07-04T12:00:00Z' }],
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

const ZONE_2: Zone = {
  id: 'zone-2',
  name: 'Office',
  lon: 3.5,
  lat: 4.5,
  radius_m: 100,
  notes: null,
  created_at: '2026-01-01T00:00:00Z',
};

const METHOD_1: AlertMethod = {
  id: 'method-1',
  name: 'My Email',
  type: 'email',
  enabled: true,
  created_at: '2026-01-01T00:00:00Z',
  config: {},
};

function baseRoutes(overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {}) {
  return {
    'GET /api/entities': () => [ENTITY_1],
    'GET /api/entities/entity-1': () => ENTITY_1_DETAIL,
    'GET /api/zones': () => [ZONE_1, ZONE_2],
    'GET /api/alert-methods': () => [METHOD_1],
    'GET /api/alert-rules': () => [] as AlertRule[],
    ...overrides,
  };
}

test('expanding an entity shows its emitters and aggregate last_seen', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));

  const detail = await screen.findByTestId('entity-detail-entity-1');
  await within(detail).findByText("Bob's Phone");
  expect(within(detail).getByText('wifi-client')).toBeInTheDocument();

  const lastSeen = within(detail).getByTestId('entity-last-seen-entity-1');
  expect(lastSeen).toHaveTextContent(new Date('2026-07-04T12:00:00Z').toLocaleString());
});

test('expanding an entity renders a detection heatmap fed by its located recent_detections', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));

  const detail = await screen.findByTestId('entity-detail-entity-1');
  expect(await within(detail).findByText('Detection heatmap')).toBeInTheDocument();
  expect(await within(detail).findByTestId('emissions-heatmap-container')).toBeInTheDocument();
});

test('an entity with no recent_detections shows the heatmap empty state', async () => {
  const noDetections: EntityDetail = { ...ENTITY_1_DETAIL, recent_detections: [] };
  const fetchMock = mockRoutes(baseRoutes({ 'GET /api/entities/entity-1': () => noDetections }));
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));

  const detail = await screen.findByTestId('entity-detail-entity-1');
  expect(await within(detail).findByText('No located detections yet.')).toBeInTheDocument();
});

test('"When detected" does not show or require a zone dropdown', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));
  await screen.findByTestId('entity-detail-entity-1');
  const addAlertButton = await screen.findByRole('button', { name: /add alert/i });

  fireEvent.click(addAlertButton);
  await screen.findByRole('heading', { name: /add alert for bob/i });

  // "When detected" is the default trigger.
  expect(screen.getByLabelText(/trigger/i)).toHaveValue('detected');
  expect(screen.queryByLabelText(/^zone$/i)).not.toBeInTheDocument();
});

test('choosing "When enters zone" reveals a zone dropdown populated from GET /api/zones', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));
  await screen.findByTestId('entity-detail-entity-1');
  const addAlertButton = await screen.findByRole('button', { name: /add alert/i });

  fireEvent.click(addAlertButton);
  await screen.findByRole('heading', { name: /add alert for bob/i });

  expect(screen.queryByLabelText(/^zone$/i)).not.toBeInTheDocument();

  fireEvent.change(screen.getByLabelText(/trigger/i), { target: { value: 'enters_zone' } });

  const zoneSelect = await screen.findByLabelText(/^zone$/i);
  expect(zoneSelect.tagName).toBe('SELECT');
  expect(within(zoneSelect).getByText('Home')).toBeInTheDocument();
  expect(within(zoneSelect).getByText('Office')).toBeInTheDocument();
});

test('enters_zone + a zone + a method, submitted, POSTs /api/alert-rules with the right body', async () => {
  const createdRule: AlertRule = {
    id: 'rule-1',
    name: 'Bob enters home',
    enabled: true,
    target_type: 'entity',
    target_id: 'entity-1',
    trigger: { on: 'enters_zone', zone_id: 'zone-1' },
    method_ids: ['method-1'],
    created_at: '2026-07-05T00:00:00Z',
  };
  const fetchMock = mockRoutes(
    baseRoutes({
      'POST /api/alert-rules': () => createdRule,
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));
  await screen.findByTestId('entity-detail-entity-1');
  const addAlertButton = await screen.findByRole('button', { name: /add alert/i });

  fireEvent.click(addAlertButton);
  await screen.findByRole('heading', { name: /add alert for bob/i });

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Bob enters home' } });
  fireEvent.change(screen.getByLabelText(/trigger/i), { target: { value: 'enters_zone' } });

  const zoneSelect = await screen.findByLabelText(/^zone$/i);
  fireEvent.change(zoneSelect, { target: { value: 'zone-1' } });

  fireEvent.click(await screen.findByLabelText(/my email/i));

  fireEvent.click(screen.getByRole('button', { name: /create alert rule/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith('/api/alert-rules', expect.objectContaining({ method: 'POST' })),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/alert-rules' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  const body = JSON.parse(init.body as string);

  expect(body).toEqual({
    name: 'Bob enters home',
    enabled: true,
    target_type: 'entity',
    target_id: 'entity-1',
    trigger: { on: 'enters_zone', zone_id: 'zone-1' },
    method_ids: ['method-1'],
  });
});

test('"When detected" submitted without a zone POSTs a body with no zone_id', async () => {
  const createdRule: AlertRule = {
    id: 'rule-2',
    name: 'Bob detected',
    enabled: true,
    target_type: 'entity',
    target_id: 'entity-1',
    trigger: { on: 'detected' },
    method_ids: ['method-1'],
    created_at: '2026-07-05T00:00:00Z',
  };
  const fetchMock = mockRoutes(
    baseRoutes({
      'POST /api/alert-rules': () => createdRule,
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());
  fireEvent.click(screen.getByRole('button', { name: 'Bob' }));
  await screen.findByTestId('entity-detail-entity-1');
  const addAlertButton = await screen.findByRole('button', { name: /add alert/i });

  fireEvent.click(addAlertButton);
  await screen.findByRole('heading', { name: /add alert for bob/i });

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Bob detected' } });
  fireEvent.click(await screen.findByLabelText(/my email/i));
  fireEvent.click(screen.getByRole('button', { name: /create alert rule/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith('/api/alert-rules', expect.objectContaining({ method: 'POST' })),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/alert-rules' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  const body = JSON.parse(init.body as string);

  expect(body).toEqual({
    name: 'Bob detected',
    enabled: true,
    target_type: 'entity',
    target_id: 'entity-1',
    trigger: { on: 'detected' },
    method_ids: ['method-1'],
  });
});

test('add entity: submitting the form POSTs /api/entities and refetches the list', async () => {
  const newEntity: Entity = { id: 'entity-2', name: 'Alice', notes: null, created_at: '2026-07-05T00:00:00Z' };
  const fetchMock = mockRoutes(
    baseRoutes({
      'GET /api/entities': () => [ENTITY_1],
      'POST /api/entities': () => newEntity,
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.click(screen.getByRole('button', { name: /add entity/i }));
  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Alice' } });
  fireEvent.click(screen.getByRole('button', { name: /^add$/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith('/api/entities', expect.objectContaining({ method: 'POST' })),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/entities' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ name: 'Alice' });
});

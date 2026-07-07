// Task 9.8 acceptance tests, pared down for Task 5's list-pages UX cleanup:
// each zone's name now links to its own deep-linkable detail page
// (`/zones/:id`, `pages/ZoneDetailPage.tsx`, see `ZoneDetailPage.test.tsx`
// for the subjects-in-zone/edit/delete coverage that used to live here
// inline). This file only covers what's still true of the list itself:
// rendering the list, and add-zone (including the client-side lat/lon/radius
// validation — out-of-range values block the POST entirely, mirroring the
// backend's own `validate_zone` range so bad input never leaves the
// browser).
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, test, vi } from 'vitest';
import Zones from './Zones';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Zone } from '../api/zones';

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>{children}</MemoryRouter>
    </QueryClientProvider>
  );
}

/** Method+pathname-aware fetch mock — same convention as
 * `Entities.test.tsx`'s `mockRoutes`. This page hits
 * `GET/POST/PATCH/DELETE /api/zones[/:id]`. */
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

const ZONE_1: Zone = {
  id: 'zone-1',
  name: 'Home',
  lon: 2.5,
  lat: 1.5,
  radius_m: 50,
  notes: 'Front yard perimeter',
  created_at: '2026-01-01T00:00:00Z',
};

const ZONE_2: Zone = {
  id: 'zone-2',
  name: 'Office',
  lon: -70.5,
  lat: 40.2,
  radius_m: 200,
  notes: null,
  created_at: '2026-01-01T00:00:00Z',
};

function baseRoutes(overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {}) {
  return {
    'GET /api/zones': () => [ZONE_1, ZONE_2],
    ...overrides,
  };
}

test('renders the zones list with name, center, and radius', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Zones />, { wrapper });

  await screen.findByTestId('zone-row-zone-1');
  expect(screen.getByText('Home')).toBeInTheDocument();
  expect(screen.getByText('Office')).toBeInTheDocument();
  expect(screen.getByTestId('zone-center-zone-1')).toHaveTextContent('1.5');
  expect(screen.getByTestId('zone-center-zone-1')).toHaveTextContent('2.5');
  expect(screen.getByTestId('zone-radius-zone-1')).toHaveTextContent('50');
});

test('a zone name links to its detail page', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Zones />, { wrapper });

  const link = await screen.findByRole('link', { name: /home/i });
  expect(link).toHaveAttribute('href', '/zones/zone-1');
});

test('add zone: submitting name/lat/lon/radius/notes POSTs /api/zones with {name, center:{lon,lat}, radius_m, notes}', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      'POST /api/zones': () => ({ ...ZONE_1, id: 'zone-3', name: 'Backyard' }),
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Zones />, { wrapper });
  await screen.findByTestId('zone-row-zone-1');

  fireEvent.click(screen.getByRole('button', { name: /add zone/i }));
  await screen.findByRole('heading', { name: /add zone/i });

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Backyard' } });
  fireEvent.change(screen.getByLabelText(/latitude/i), { target: { value: '12.5' } });
  fireEvent.change(screen.getByLabelText(/longitude/i), { target: { value: '-45.25' } });
  fireEvent.change(screen.getByLabelText(/radius/i), { target: { value: '150' } });
  fireEvent.change(screen.getByLabelText(/notes/i), { target: { value: 'Behind the shed' } });

  fireEvent.click(screen.getByRole('button', { name: /^add$/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith('/api/zones', expect.objectContaining({ method: 'POST' })),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/zones' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  const body = JSON.parse(init.body as string);

  expect(body).toEqual({
    name: 'Backyard',
    center: { lon: -45.25, lat: 12.5 },
    radius_m: 150,
    notes: 'Behind the shed',
  });
  expect(typeof body.center.lon).toBe('number');
  expect(typeof body.center.lat).toBe('number');
  expect(typeof body.radius_m).toBe('number');
});

test('an out-of-range latitude shows a validation error and does not POST', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Zones />, { wrapper });
  await screen.findByTestId('zone-row-zone-1');

  fireEvent.click(screen.getByRole('button', { name: /add zone/i }));
  await screen.findByRole('heading', { name: /add zone/i });

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Bad Zone' } });
  fireEvent.change(screen.getByLabelText(/latitude/i), { target: { value: '200' } });
  fireEvent.change(screen.getByLabelText(/longitude/i), { target: { value: '10' } });
  fireEvent.change(screen.getByLabelText(/radius/i), { target: { value: '50' } });

  fireEvent.click(screen.getByRole('button', { name: /^add$/i }));

  expect(await screen.findByRole('alert')).toHaveTextContent(/latitude/i);
  expect(fetchMock).not.toHaveBeenCalledWith('/api/zones', expect.objectContaining({ method: 'POST' }));
});

test('a zero radius shows a validation error and does not POST', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Zones />, { wrapper });
  await screen.findByTestId('zone-row-zone-1');

  fireEvent.click(screen.getByRole('button', { name: /add zone/i }));
  await screen.findByRole('heading', { name: /add zone/i });

  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'Bad Zone' } });
  fireEvent.change(screen.getByLabelText(/latitude/i), { target: { value: '10' } });
  fireEvent.change(screen.getByLabelText(/longitude/i), { target: { value: '10' } });
  fireEvent.change(screen.getByLabelText(/radius/i), { target: { value: '0' } });

  fireEvent.click(screen.getByRole('button', { name: /^add$/i }));

  expect(await screen.findByRole('alert')).toHaveTextContent(/radius/i);
  expect(fetchMock).not.toHaveBeenCalledWith('/api/zones', expect.objectContaining({ method: 'POST' }));
});

// Task 9.6 acceptance tests, pared down for Task 4's list-pages UX cleanup:
// each entity's name now links to its own deep-linkable detail page
// (`/entities/:id`, `pages/EntityDetailPage.tsx`, see `EntityDetailPage.test.tsx`
// for the expanded-detail/add-alert coverage that used to live here inline).
// This file only covers what's still true of the list itself: search,
// pagination, mass-select/bulk-delete/clear-all, and add-entity.
import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, test, vi } from 'vitest';
import Entities from './Entities';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Entity } from '../api/entities';

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
 * `DataSources.test.tsx`'s `mockMethodRoutes` / `Emitters.test.tsx`'s
 * `mockRoutes`. The list page itself only hits `GET/POST /api/entities` plus
 * the bulk-delete/clear endpoints. */
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

function baseRoutes(overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {}) {
  return {
    'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
    ...overrides,
  };
}

test('an entity name links to its detail page', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });

  const link = await screen.findByRole('link', { name: /bob/i });
  expect(link).toHaveAttribute('href', '/entities/entity-1');
});

// --- Phase 4: search + pagination + mass-select/bulk-delete/clear-all ---

const ENTITY_2: Entity = {
  id: 'entity-2',
  name: 'Alice',
  notes: null,
  created_at: '2026-06-02T00:00:00Z',
};

test('typing in the search bar refetches with the search param', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.change(screen.getByPlaceholderText('Search entities…'), { target: { value: 'bob' } });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), 'http://localhost');
      return url.pathname === '/api/entities' && url.searchParams.get('search') === 'bob';
    });
    expect(call).toBeDefined();
  });
});

test('pagination (Next) refetches with the next offset and clears the row selection', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      'GET /api/entities': (url) =>
        url.searchParams.get('offset') === '50'
          ? { items: [ENTITY_2], total: 60 }
          : { items: [ENTITY_1], total: 60 },
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText('Select entity entity-1'));
  expect(screen.getByRole('button', { name: /delete selected \(1\)/i })).toBeEnabled();

  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-2')).toBeInTheDocument());

  expect(screen.getByRole('button', { name: /delete selected \(0\)/i })).toBeDisabled();
});

test('checking a row and clicking "Delete selected" POSTs bulk-delete with that id (after confirm)', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      'POST /api/entities/bulk-delete': () => ({ deleted: 1 }),
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText('Select entity entity-1'));
  fireEvent.click(screen.getByRole('button', { name: /delete selected \(1\)/i }));

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), 'http://localhost').pathname === '/api/entities/bulk-delete' &&
        init?.method === 'POST',
    );
    expect(call).toBeDefined();
  });
  const [, init] = fetchMock.mock.calls.find(
    ([input]) => new URL(String(input), 'http://localhost').pathname === '/api/entities/bulk-delete',
  ) as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ ids: ['entity-1'] });
});

test('"Clear All Entities" POSTs clear after a confirm', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      'POST /api/entities/clear': () => ({ deleted: 1 }),
    }),
  );
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.click(screen.getByRole('button', { name: /clear all entities/i }));

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), 'http://localhost').pathname === '/api/entities/clear' && init?.method === 'POST',
    );
    expect(call).toBeDefined();
  });
});

test('declining the confirm dialog does not call bulk-delete', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(false);
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal('fetch', fetchMock);

  render(<Entities />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('entity-row-entity-1')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText('Select entity entity-1'));
  fireEvent.click(screen.getByRole('button', { name: /delete selected \(1\)/i }));

  await waitFor(() => expect(window.confirm).toHaveBeenCalled());
  expect(fetchMock.mock.calls.find(([input]) => String(input).includes('bulk-delete'))).toBeUndefined();
});

test('add entity: submitting the form POSTs /api/entities and refetches the list', async () => {
  const newEntity: Entity = { id: 'entity-2', name: 'Alice', notes: null, created_at: '2026-07-05T00:00:00Z' };
  const fetchMock = mockRoutes(
    baseRoutes({
      'GET /api/entities': () => ({ items: [ENTITY_1], total: 1 }),
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

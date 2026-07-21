import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, test, vi } from 'vitest';
import AppShell from './AppShell';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { AppConfig } from '../api/client';

afterEach(() => vi.unstubAllGlobals());

function renderShell(config: AppConfig) {
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (url.includes('/api/config')) return Promise.resolve(jsonResponse(config));
      return Promise.resolve(jsonResponse({ items: [], total: 0, unread_count: 0 }));
    }),
  );
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <AppShell onLogout={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test('standalone shows the full analysis nav', async () => {
  renderShell({ role: 'standalone', node_sensor_id: 'local' });
  expect(await screen.findByRole('link', { name: 'Emitters' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Entities' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Zones' })).toBeInTheDocument();
});

test('sensor shows only Dashboard, Data Sources, Emissions', async () => {
  renderShell({ role: 'sensor', node_sensor_id: 'frontgate' });
  // The nav defaults to the full (standalone) item set while `useConfig`'s
  // fetch is in flight, then re-renders once the sensor role resolves — so
  // wait for that settle (Emitters disappearing) before asserting on the
  // narrowed set, rather than a one-shot `queryByRole` that could observe
  // the pre-load render.
  await waitFor(() => expect(screen.queryByRole('link', { name: 'Emitters' })).not.toBeInTheDocument());
  expect(screen.getByRole('link', { name: 'Dashboard' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Data Sources' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Emissions' })).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Entities' })).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Co-Travel' })).not.toBeInTheDocument();
});

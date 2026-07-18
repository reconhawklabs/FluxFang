import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, test, vi } from 'vitest';
import AiAuditLog from './AiAuditLog';
import { jsonResponse } from '../test-utils/fetchMocks';

afterEach(() => vi.unstubAllGlobals());

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>{children}</MemoryRouter>
    </QueryClientProvider>
  );
}

test('renders audit rows with tool, action badge and summary', async () => {
  const page = {
    items: [
      { id: 'a1', created_at: '2026-07-17T00:00:00Z', tool: 'create_entity', action: 'add',
        summary: 'Created entity Silver Sedan', args: {}, result: null, affected_ids: [], status: 'ok', error: null },
      { id: 'a2', created_at: '2026-07-17T00:01:00Z', tool: 'delete_emitter', action: 'remove',
        summary: 'Deleted emitter X', args: {}, result: null, affected_ids: [], status: 'ok', error: null },
    ],
    total: 2,
  };
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(jsonResponse(page))));

  render(<AiAuditLog />, { wrapper });

  await screen.findByTestId('audit-row-a1');
  expect(screen.getByText('create_entity')).toBeInTheDocument();
  expect(screen.getByText('Created entity Silver Sedan')).toBeInTheDocument();
  expect(screen.getByTestId('audit-action-a2')).toHaveTextContent('remove');
});

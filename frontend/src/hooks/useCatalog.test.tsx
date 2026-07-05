import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useCatalog } from './useCatalog';
import { mockFetchRoutes } from '../test-utils/fetchMocks';

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient();
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

test('fetches GET /api/catalog/:kind and returns the field defs', async () => {
  const fetchMock = mockFetchRoutes({
    '/api/catalog/wifi': [
      { key: 'bssid', label: 'BSSID', type: 'mac', ops: [{ code: 'eq', label: 'is exactly' }] },
    ],
  });
  vi.stubGlobal('fetch', fetchMock);

  const { result } = renderHook(() => useCatalog('wifi'), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));

  expect(result.current.data).toEqual([
    { key: 'bssid', label: 'BSSID', type: 'mac', ops: [{ code: 'eq', label: 'is exactly' }] },
  ]);
  expect(fetchMock).toHaveBeenCalledWith('/api/catalog/wifi', expect.objectContaining({ method: 'GET' }));
});

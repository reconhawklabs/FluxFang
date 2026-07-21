import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Sensors from './Sensors';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { Sensor } from '../api/sensors';

afterEach(() => vi.unstubAllGlobals());

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

const PENDING: Sensor = {
  id: 's1', data_source_id: 'ds1', sensor_id: 'frontgate', fingerprint: '4F-A2-09-EE',
  status: 'pending', auto_group_emitters: true, source_ip: '5.6.7.8',
  approved_at: null, last_seen_at: '2026-07-21T00:00:00Z', online: true,
};

function mockSensors(list: Sensor[]) {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse(list));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(`unexpected ${url}`));
  }));
}

test('renders empty pending + registered sections', async () => {
  mockSensors([]);
  render(<Sensors />, { wrapper });
  await waitFor(() => expect(screen.getByText('No approved sensors yet.')).toBeInTheDocument());
  expect(screen.getByText('No sensors awaiting approval.')).toBeInTheDocument();
});

test('counts a pending sensor', async () => {
  mockSensors([PENDING]);
  render(<Sensors />, { wrapper });
  await waitFor(() => expect(screen.getByText('1 pending')).toBeInTheDocument());
});

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
  await waitFor(() => expect(screen.getByTestId('pending-frontgate')).toBeInTheDocument());
});

test('approve dialog shows the fingerprint and posts the auto_group choice', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST') {
      calls.push({ url, body: init.body ? JSON.parse(init.body as string) : null });
      return Promise.resolve(jsonResponse({ ...PENDING, status: 'approved' }));
    }
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([PENDING]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));

  const { default: SensorsPage } = await import('./Sensors');
  render(<SensorsPage />, { wrapper });

  fireEvent.click(await screen.findByRole('button', { name: /approve/i }));
  // dialog shows the fingerprint for out-of-band confirmation. Scoped to the
  // dialog: the pending-list row also renders the fingerprint (as "fp
  // 4F-A2-09-EE"), so an unscoped getByText matches both and throws.
  const dialog = screen.getByRole('dialog');
  expect(within(dialog).getByText(/4F-A2-09-EE/)).toBeInTheDocument();
  // toggle auto-group OFF (defaults on) then confirm
  fireEvent.click(within(dialog).getByLabelText(/group emissions into emitters/i));
  fireEvent.click(within(dialog).getByRole('button', { name: /confirm/i }));

  await waitFor(() => expect(calls.some((c) => c.url.includes('/approve'))).toBe(true));
  const approve = calls.find((c) => c.url.includes('/approve'))!;
  expect(approve.body).toEqual({ auto_group_emitters: false });
});

test('reject posts to the reject endpoint', async () => {
  const calls: string[] = [];
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST') { calls.push(url); return Promise.resolve(jsonResponse({ ...PENDING, status: 'rejected' })); }
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([PENDING]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /reject/i }));
  await waitFor(() => expect(calls.some((u) => u.includes('/reject'))).toBe(true));
});

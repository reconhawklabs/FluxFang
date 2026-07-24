// The Sensor dashboard's forwarding tile.
//
// It exists because reachability and forwarding are different questions and
// the UI used to answer only the first: a sensor whose every batch was
// failing showed "Connected" while the Standalone listed it as down. These
// pin that the two are now reported separately, and that a broken forwarding
// loop cannot render as healthy.
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, test, vi } from 'vitest';
import SensorDashboard from './SensorDashboard';
import type { ForwarderStatus, SensorStatus } from '../api/client';

function wrapper({ children }: { children: React.ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function jsonResponse(body: unknown): Response {
  return { ok: true, status: 200, statusText: 'OK', text: () => Promise.resolve(JSON.stringify(body)) } as Response;
}

const FORWARDING: ForwarderStatus = {
  state: 'forwarding',
  last_contact_at: '2026-07-23T00:00:00Z',
  last_delivery_at: '2026-07-23T00:00:00Z',
  delivered_since_start: 1200,
  last_error: null,
};

function statusWith(forwarding: ForwarderStatus, connected = true): SensorStatus {
  return {
    role: 'sensor',
    node_sensor_id: 'frontgate',
    cache: { total: 10, undelivered: 0 },
    delivered_last_hour: 500,
    connected,
    forwarding,
    sensor: { host: 'base.local', port: 9000 },
  };
}

function stubStatus(status: SensorStatus) {
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (url.includes('/api/sensor/status')) return Promise.resolve(jsonResponse(status));
      if (url.includes('/api/cached-emissions')) return Promise.resolve(jsonResponse([]));
      return Promise.reject(new Error(url));
    }),
  );
}

afterEach(() => vi.unstubAllGlobals());

test('a healthy sensor reports that it is delivering', async () => {
  stubStatus(statusWith(FORWARDING));
  render(<SensorDashboard />, { wrapper });
  // The tile is in the DOM before the query resolves, so wait on the text
  // rather than the element -- asserting on the element would race the fetch.
  expect(await screen.findByText('Delivering')).toBeInTheDocument();
  expect(screen.getByTestId('forwarding-state')).toHaveTextContent('Delivering');
});

test('a reachable Standalone with failing batches reports Failing, not Delivering', async () => {
  // The exact contradiction that made this failure so hard to read: the
  // listener answers, so `connected` is true, but nothing is getting through.
  stubStatus(statusWith({ ...FORWARDING, last_error: 'ingest status 500' }, true));
  render(<SensorDashboard />, { wrapper });
  expect(await screen.findByText('Failing')).toBeInTheDocument();
  expect(screen.getByTestId('forwarding-error')).toHaveTextContent('ingest status 500');
  // ...while reachability still reads as fine, which is the useful signal:
  // the two disagreeing localises the fault to ingest rather than the network.
  expect(screen.getByText('Reachable')).toBeInTheDocument();
});

test('the approval prompt is offered while approval is still outstanding', async () => {
  stubStatus(statusWith({ ...FORWARDING, state: 'enrolling', last_error: null }));
  render(<SensorDashboard />, { wrapper });
  expect(await screen.findByTestId('approval-prompt')).toBeInTheDocument();
  expect(screen.getByTestId('request-approval')).toBeInTheDocument();
});

test('the approval prompt disappears once forwarding is under way', async () => {
  // Nothing left to request at this point, so offering it would only invite
  // pointless round trips.
  stubStatus(statusWith(FORWARDING));
  render(<SensorDashboard />, { wrapper });
  expect(await screen.findByText('Delivering')).toBeInTheDocument();
  expect(screen.queryByTestId('approval-prompt')).not.toBeInTheDocument();
});

test('a sensor still awaiting operator approval says so', async () => {
  stubStatus(
    statusWith({
      ...FORWARDING,
      state: 'enrolling',
      last_error: null,
    }),
  );
  render(<SensorDashboard />, { wrapper });
  expect(await screen.findByText('Awaiting approval')).toBeInTheDocument();
});

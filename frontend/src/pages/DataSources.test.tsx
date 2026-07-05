import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import DataSources from './DataSources';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { DataSource } from '../api/dataSources';

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

/** Method-aware fetch mock — `test-utils/fetchMocks`'s `mockFetchRoutes`
 * keys purely on pathname, which can't distinguish e.g. `GET
 * /api/data-sources` (list) from `POST /api/data-sources` (create) that
 * this page's tests both need against the same path. */
function mockMethodRoutes(handlers: Record<string, (url: URL, init?: RequestInit) => unknown>) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const raw = typeof input === 'string' ? input : input.toString();
    const url = new URL(raw, 'http://localhost');
    const method = (init?.method ?? 'GET').toUpperCase();
    const key = `${method} ${url.pathname}`;
    const handler = handlers[key];
    if (!handler) {
      return Promise.reject(new Error(`mockMethodRoutes: no route registered for ${key}`));
    }
    return Promise.resolve(jsonResponse(handler(url, init)));
  });
}

const SOURCES: DataSource[] = [
  {
    id: 'wifi-1',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'wifi',
    mode: 'monitor',
    interface: 'wlan0',
    status: 'stopped',
    config: {},
    last_error: null,
  },
  {
    id: 'wifi-2',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'wifi',
    mode: 'monitor',
    interface: 'wlan1',
    status: 'starting',
    config: {},
    last_error: null,
  },
  {
    id: 'gps-1',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'gpsd',
    interface: null,
    status: 'running',
    config: { host: '127.0.0.1', port: 2947 },
    last_error: null,
  },
  {
    id: 'gps-2',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'serial',
    interface: null,
    status: 'error',
    config: { device: '/dev/ttyUSB0', baud: 9600 },
    last_error: 'device not found',
  },
];

test('renders the source list with color-coded status badges and last_error', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => SOURCES,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('source-row-wifi-1')).toBeInTheDocument());

  expect(within(screen.getByTestId('source-row-wifi-1')).getByTestId('status-badge')).toHaveTextContent(/stopped/i);
  expect(within(screen.getByTestId('source-row-wifi-2')).getByTestId('status-badge')).toHaveTextContent(/starting/i);
  expect(within(screen.getByTestId('source-row-gps-1')).getByTestId('status-badge')).toHaveTextContent(/running/i);
  expect(within(screen.getByTestId('source-row-gps-2')).getByTestId('status-badge')).toHaveTextContent(/error/i);

  // interface/device rendered in monospace
  expect(screen.getByText('wlan0')).toHaveClass('font-mono');
  expect(screen.getByText('/dev/ttyUSB0')).toHaveClass('font-mono');

  // last_error surfaced only for the errored source
  expect(within(screen.getByTestId('source-row-gps-2')).getByText(/device not found/i)).toBeInTheDocument();
});

test('add source: wifi kind reveals an interface text input', async () => {
  const fetchMock = mockMethodRoutes({ 'GET /api/data-sources': () => [] });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  // wifi is the default kind
  expect(screen.getByLabelText(/interface/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/interface/i).tagName).toBe('INPUT');
});

test('add source: gps + gpsd mode reveals host and port fields', async () => {
  const fetchMock = mockMethodRoutes({ 'GET /api/data-sources': () => [] });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'gps' } });
  // gpsd is the default gps mode
  expect(screen.getByLabelText(/host/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/port/i)).toBeInTheDocument();
});

test('add source: gps + serial mode reveals a device input and a baud DROPDOWN (not free text), and posts the chosen config', async () => {
  const created: DataSource = {
    id: 'new-1',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'serial',
    interface: null,
    status: 'stopped',
    config: { device: '/dev/ttyUSB1', baud: 57600 },
    last_error: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'gps' } });
  fireEvent.change(screen.getByLabelText(/^mode$/i), { target: { value: 'serial' } });

  const deviceInput = screen.getByLabelText(/device/i);
  expect(deviceInput.tagName).toBe('INPUT');
  fireEvent.change(deviceInput, { target: { value: '/dev/ttyUSB1' } });

  const baudField = screen.getByLabelText(/baud/i);
  // The critical assertion: baud is a <select> dropdown, not a free-text input.
  expect(baudField.tagName).toBe('SELECT');
  const options = within(baudField as HTMLSelectElement).getAllByRole('option').map((o) => o.textContent);
  expect(options).toEqual(['4800', '9600', '19200', '38400', '57600', '115200']);
  fireEvent.change(baudField, { target: { value: '57600' } });

  fireEvent.click(screen.getByRole('button', { name: /^add$|^create$|^save$/i }));

  await waitFor(() => expect(screen.queryByLabelText(/device/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'gps',
    mode: 'serial',
    config: { device: '/dev/ttyUSB1', baud: 57600 },
  });
});

test('Start button on a stopped source calls the start endpoint', async () => {
  const stopped = SOURCES[0]; // wifi-1, status stopped
  const started: DataSource = { ...stopped, status: 'starting' };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [stopped],
    'POST /api/data-sources/wifi-1/start': () => started,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('source-row-wifi-1')).toBeInTheDocument());

  const row = screen.getByTestId('source-row-wifi-1');
  fireEvent.click(within(row).getByRole('button', { name: /start/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/data-sources/wifi-1/start',
      expect.objectContaining({ method: 'POST' }),
    ),
  );
});

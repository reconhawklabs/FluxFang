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
    desired_state: 'stopped',
    last_ok_at: null,
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
    desired_state: 'running',
    last_ok_at: null,
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
    desired_state: 'running',
    last_ok_at: '2026-01-01T00:05:00Z',
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
    // desired_state stays 'running' — the reconciler is retrying every 10s
    // (see backend module docs); last_ok_at anchors the "down for" duration.
    desired_state: 'running',
    last_ok_at: '2026-01-01T00:00:00Z',
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

  // Health column: an errored source whose desired_state is still 'running'
  // (the reconciler is retrying) reads honestly as "retrying", not a bare
  // "error" that could be mistaken for something requiring manual action.
  expect(within(screen.getByTestId('source-row-gps-2')).getByText(/retrying/i)).toBeInTheDocument();
  // ...while a running source reads as healthy.
  expect(within(screen.getByTestId('source-row-gps-1')).getByText(/healthy/i)).toBeInTheDocument();
});

/** `GET /api/system/capture-devices` fixture builder — the Add form fetches
 * this whenever it's mounted (wifi, gps-serial, and bluetooth paths all
 * depend on it), so every Add-form test below registers a route for it. */
function captureDevices(
  wifiInterfaces: string[],
  serialDevices: string[],
  bluetoothInterfaces: string[] = [],
  rtlSdrDevices: { index: number; name: string; serial: string }[] = [],
) {
  return {
    wifi_interfaces: wifiInterfaces,
    serial_devices: serialDevices,
    bluetooth_interfaces: bluetoothInterfaces,
    rtl_sdr_devices: rtlSdrDevices,
  };
}

test('add source: wifi kind with enumerated interfaces shows a Mode dropdown and an interface SELECT (not free text), and posts the chosen mode/interface', async () => {
  const created: DataSource = {
    id: 'new-wifi',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'wifi',
    mode: 'scan',
    interface: 'wlan2',
    status: 'stopped',
    config: {},
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices(['wlan0', 'wlan2'], []),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  // wifi is the default kind

  const modeField = await screen.findByLabelText(/^mode$/i);
  expect(modeField.tagName).toBe('SELECT');
  const modeOptions = within(modeField as HTMLSelectElement).getAllByRole('option').map((o) => o.textContent);
  expect(modeOptions).toEqual(['Monitor Mode', 'SSID Scan']);

  const ifaceField = await screen.findByLabelText(/interface/i);
  // The critical assertion: interface is a <select> dropdown, not free text.
  expect(ifaceField.tagName).toBe('SELECT');
  const ifaceOptions = within(ifaceField as HTMLSelectElement)
    .getAllByRole('option')
    .map((o) => o.textContent);
  expect(ifaceOptions).toEqual(['Select an interface…', 'wlan0', 'wlan2']);

  fireEvent.change(modeField, { target: { value: 'scan' } });
  fireEvent.change(ifaceField, { target: { value: 'wlan2' } });

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).not.toBeDisabled();
  fireEvent.click(submitButton);

  await waitFor(() => expect(screen.queryByLabelText(/interface/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'wifi',
    mode: 'scan',
    interface: 'wlan2',
    config: { auto_create_emitters: false },
  });
});

test('add source: wifi kind — checking "Automatically create emitters" posts config.auto_create_emitters: true', async () => {
  const created: DataSource = {
    id: 'new-wifi-auto',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'wifi',
    mode: 'monitor',
    interface: 'wlan0',
    status: 'stopped',
    config: { auto_create_emitters: true },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices(['wlan0'], []),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  // wifi + monitor are the defaults

  const ifaceField = await screen.findByLabelText(/interface/i);
  fireEvent.change(ifaceField, { target: { value: 'wlan0' } });

  const autoCreateCheckbox = screen.getByLabelText(/automatically create emitters/i);
  expect(autoCreateCheckbox).not.toBeChecked();
  fireEvent.click(autoCreateCheckbox);
  expect(autoCreateCheckbox).toBeChecked();

  fireEvent.click(screen.getByRole('button', { name: /^add$|^create$|^save$/i }));

  await waitFor(() => expect(screen.queryByLabelText(/interface/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'wifi',
    mode: 'monitor',
    interface: 'wlan0',
    config: { auto_create_emitters: true },
  });
});

test('add source on a Sensor node: hides the "Sensor" kind and the auto-create-emitters toggle', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices(['wlan0'], []),
    'GET /api/config': () => ({ role: 'sensor', node_sensor_id: 'edge-1' }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  await screen.findByLabelText(/interface/i); // wifi (default) form rendered

  // A Sensor never hosts a listener nor builds emitters — both are gone once
  // its role config resolves.
  await waitFor(() =>
    expect(screen.queryByLabelText(/automatically create emitters/i)).not.toBeInTheDocument(),
  );
  expect(
    screen.queryByRole('option', { name: /sensor \(listener\)/i }),
  ).not.toBeInTheDocument();
});

test('add source: wifi kind — SSID Scan mode also shows the auto-create checkbox', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices(['wlan0'], []),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(await screen.findByLabelText(/^mode$/i), { target: { value: 'scan' } });

  expect(screen.getByLabelText(/automatically create emitters/i)).toBeInTheDocument();
});

test('add source: bluetooth kind — picking an adapter and enabling Active Scanning posts the scan config', async () => {
  const created: DataSource = {
    id: 'new-bt',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'bluetooth',
    mode: 'scan',
    interface: 'hci0',
    status: 'stopped',
    config: { auto_create_emitters: false, active_scan: true },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], [], ['hci0']),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'bluetooth' } });

  const ifaceField = await screen.findByLabelText(/adapter/i);
  expect(ifaceField.tagName).toBe('SELECT');
  const ifaceOptions = within(ifaceField as HTMLSelectElement)
    .getAllByRole('option')
    .map((o) => o.textContent);
  expect(ifaceOptions).toEqual(['Select an adapter…', 'hci0']);
  fireEvent.change(ifaceField, { target: { value: 'hci0' } });

  const activeScanCheckbox = screen.getByLabelText(/enable active scanning/i);
  expect(activeScanCheckbox).not.toBeChecked();
  fireEvent.click(activeScanCheckbox);
  expect(activeScanCheckbox).toBeChecked();

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).not.toBeDisabled();
  fireEvent.click(submitButton);

  await waitFor(() => expect(screen.queryByLabelText(/adapter/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'bluetooth',
    mode: 'scan',
    interface: 'hci0',
    config: { auto_create_emitters: false, active_scan: true },
  });
});

test('add source: bluetooth kind — active scanning help text explains the RF consequences of the toggle', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], [], ['hci0']),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'bluetooth' } });

  await screen.findByLabelText(/adapter/i);

  expect(screen.getByText(/transmit scan-request frames/i)).toBeInTheDocument();
  expect(screen.getByText(/RF-quiet/i)).toBeInTheDocument();
  expect(screen.getByText(/some adapters may fail/i)).toBeInTheDocument();
});

test('add source: bluetooth kind with NO enumerated adapters shows "No Bluetooth adapter found." and disables the Add button', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], [], []),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'bluetooth' } });

  expect(await screen.findByText('No Bluetooth adapter found.')).toBeInTheDocument();
  expect(screen.queryByLabelText(/adapter/i)).not.toBeInTheDocument();

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).toBeDisabled();
});

test('add source: rtl_sdr kind — submits an rtl_sdr/tpms source with frequency, device_serial, and toggles', async () => {
  const created: DataSource = {
    id: 'new-rtl',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'rtl_sdr',
    mode: 'tpms',
    interface: null,
    status: 'stopped',
    config: {
      frequency: '433.92M',
      device_serial: '67475624',
      auto_create_emitters: true,
      auto_correlate_tpms: true,
    },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () =>
      captureDevices([], [], [], [{ index: 0, name: 'Nooelec NESDR', serial: '67475624' }]),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'rtl_sdr' } });

  const frequencyField = await screen.findByLabelText(/frequency/i);
  expect(frequencyField.tagName).toBe('SELECT');
  fireEvent.change(frequencyField, { target: { value: '433.92M' } });

  const deviceField = screen.getByLabelText(/device/i);
  expect(deviceField.tagName).toBe('SELECT');
  const deviceOptions = within(deviceField as HTMLSelectElement)
    .getAllByRole('option')
    .map((o) => o.textContent);
  expect(deviceOptions).toEqual(['Select a device…', 'index 0 — Nooelec NESDR (SN: 67475624)']);
  fireEvent.change(deviceField, { target: { value: '67475624' } });

  const autoCreateCheckbox = screen.getByLabelText(/automatically create emitters/i);
  expect(autoCreateCheckbox).not.toBeChecked();
  fireEvent.click(autoCreateCheckbox);
  expect(autoCreateCheckbox).toBeChecked();

  const autoCorrelateCheckbox = screen.getByLabelText(/attempt to connect tpms emitters to other tires/i);
  expect(autoCorrelateCheckbox).not.toBeChecked();
  fireEvent.click(autoCorrelateCheckbox);
  expect(autoCorrelateCheckbox).toBeChecked();

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).not.toBeDisabled();
  fireEvent.click(submitButton);

  await waitFor(() => expect(screen.queryByLabelText(/frequency/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'rtl_sdr',
    mode: 'tpms',
    config: {
      frequency: '433.92M',
      device_serial: '67475624',
      auto_create_emitters: true,
      auto_correlate_tpms: true,
    },
  });
});

test('add source: wifi kind with NO enumerated interfaces shows "No compatible WiFi card found." and disables the Add button, with no text input fallback', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], []),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));

  expect(await screen.findByText('No compatible WiFi card found.')).toBeInTheDocument();
  expect(screen.queryByLabelText(/interface/i)).not.toBeInTheDocument();

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).toBeDisabled();
});

test('add source: gps + gpsd mode reveals host and port fields', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], []),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'gps' } });
  // gpsd is the default gps mode
  expect(screen.getByLabelText(/host/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/port/i)).toBeInTheDocument();
});

test('add source: gps + serial mode reveals a device SELECT (not free text) and a baud DROPDOWN, and posts the chosen config', async () => {
  const created: DataSource = {
    id: 'new-1',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'serial',
    interface: null,
    status: 'stopped',
    config: { device: '/dev/ttyUSB1', baud: 57600 },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], ['/dev/ttyUSB0', '/dev/ttyUSB1']),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'gps' } });
  fireEvent.change(screen.getByLabelText(/^mode$/i), { target: { value: 'serial' } });

  const deviceField = await screen.findByLabelText(/device/i);
  // The critical assertion: device is a <select> dropdown, not free text.
  expect(deviceField.tagName).toBe('SELECT');
  const deviceOptions = within(deviceField as HTMLSelectElement)
    .getAllByRole('option')
    .map((o) => o.textContent);
  expect(deviceOptions).toEqual(['Select a device…', '/dev/ttyUSB0', '/dev/ttyUSB1']);
  fireEvent.change(deviceField, { target: { value: '/dev/ttyUSB1' } });

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

test('add source: gps + serial mode with NO enumerated serial devices shows "No compatible serial GPS device found." and disables Add', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices(['wlan0'], []),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'gps' } });
  fireEvent.change(screen.getByLabelText(/^mode$/i), { target: { value: 'serial' } });

  expect(await screen.findByText('No compatible serial GPS device found.')).toBeInTheDocument();
  expect(screen.queryByLabelText(/device/i)).not.toBeInTheDocument();

  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).toBeDisabled();
});

test('Edit button appears only for a stopped manual gps source, not a running one', async () => {
  const manualStopped: DataSource = {
    id: 'gps-manual-stopped',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'manual',
    interface: null,
    status: 'stopped',
    config: { lat: 37.7749, lon: -122.4194 },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const manualRunning: DataSource = {
    ...manualStopped,
    id: 'gps-manual-running',
    status: 'running',
    desired_state: 'running',
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [manualStopped, manualRunning],
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('source-row-gps-manual-stopped')).toBeInTheDocument());

  expect(
    within(screen.getByTestId('source-row-gps-manual-stopped')).getByRole('button', { name: /^edit$/i }),
  ).toBeInTheDocument();
  expect(
    within(screen.getByTestId('source-row-gps-manual-running')).queryByRole('button', { name: /^edit$/i }),
  ).not.toBeInTheDocument();
});

test('Edit modal pre-fills lat/lon and PATCHes the source with the new values on save', async () => {
  const manual: DataSource = {
    id: 'gps-manual-1',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'gps',
    mode: 'manual',
    interface: null,
    status: 'stopped',
    config: { lat: 37.7749, lon: -122.4194 },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const updated: DataSource = { ...manual, config: { lat: 40.7128, lon: -74.006 } };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [manual],
    'PATCH /api/data-sources/gps-manual-1': () => updated,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('source-row-gps-manual-1')).toBeInTheDocument());

  fireEvent.click(
    within(screen.getByTestId('source-row-gps-manual-1')).getByRole('button', { name: /^edit$/i }),
  );

  const latField = await screen.findByLabelText(/latitude/i);
  const lonField = screen.getByLabelText(/longitude/i);
  expect(latField).toHaveValue(37.7749);
  expect(lonField).toHaveValue(-122.4194);

  fireEvent.change(latField, { target: { value: '40.7128' } });
  fireEvent.change(lonField, { target: { value: '-74.006' } });

  fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

  await waitFor(() => expect(screen.queryByLabelText(/latitude/i)).not.toBeInTheDocument());

  const patchCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');
  expect(patchCall).toBeDefined();
  const [url, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources/gps-manual-1');
  expect(JSON.parse(init.body as string)).toEqual({
    mode: 'manual',
    config: { lat: 40.7128, lon: -74.006 },
  });
});

test('add source: sensor kind — submits a sensor/listener source with bind_ip/bind_port', async () => {
  const created: DataSource = {
    id: 'new-sensor',
    created_at: '2026-01-01T00:00:00Z',
    kind: 'sensor',
    mode: 'listener',
    interface: null,
    status: 'stopped',
    config: { bind_ip: '0.0.0.0', bind_port: 9000 },
    last_error: null,
    desired_state: 'stopped',
    last_ok_at: null,
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/data-sources': () => [],
    'GET /api/system/capture-devices': () => captureDevices([], []),
    'POST /api/data-sources': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<DataSources />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add data source/i }));
  fireEvent.change(screen.getByLabelText(/^kind$/i), { target: { value: 'sensor' } });

  // Sensor is a pure network bind — no hardware enumeration/select shown.
  expect(await screen.findByLabelText(/bind ip/i)).toBeInTheDocument();
  expect(screen.queryByText(/no compatible/i)).not.toBeInTheDocument();

  // Defaults are pre-filled and already valid.
  const submitButton = screen.getByRole('button', { name: /^add$|^create$|^save$/i });
  expect(submitButton).not.toBeDisabled();
  fireEvent.click(submitButton);

  await waitFor(() => expect(screen.queryByLabelText(/bind ip/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe('/api/data-sources');
  expect(JSON.parse(init.body as string)).toEqual({
    kind: 'sensor',
    mode: 'listener',
    config: { bind_ip: '0.0.0.0', bind_port: 9000 },
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

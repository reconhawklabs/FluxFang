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
  id: 's1', data_source_id: 'ds1', sensor_id: 'frontgate', fingerprint: '4F-A2-09-EE-1B-77-C3-90',
  status: 'pending', auto_group_emitters: true, source_ip: '5.6.7.8',
  approved_at: null, last_seen_at: '2026-07-21T00:00:00Z', online: true, emissions_24h: 0,
};

// A well-formed 32-byte base64 key (btoa of 32 ASCII bytes) — passes the
// dialog's `isValidKey` shape check so Confirm enables.
const VALID_KEY = btoa('0123456789abcdef0123456789abcdef');

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
  // Confirm stays disabled until the operator types a valid-shaped key.
  const confirm = within(dialog).getByRole('button', { name: /confirm/i });
  expect(confirm).toBeDisabled();
  // A malformed key keeps Confirm disabled and surfaces a shape hint.
  fireEvent.change(within(dialog).getByLabelText(/sensor encryption key/i), { target: { value: 'not-a-key' } });
  expect(confirm).toBeDisabled();
  expect(within(dialog).getByText(/doesn't look like a valid key/i)).toBeInTheDocument();
  fireEvent.change(within(dialog).getByLabelText(/sensor encryption key/i), { target: { value: VALID_KEY } });
  // toggle auto-group OFF (defaults on) then confirm
  fireEvent.click(within(dialog).getByLabelText(/group emissions into emitters/i));
  expect(confirm).not.toBeDisabled();
  fireEvent.click(confirm);

  await waitFor(() => expect(calls.some((c) => c.url.includes('/approve'))).toBe(true));
  const approve = calls.find((c) => c.url.includes('/approve'))!;
  expect(approve.body).toEqual({ auto_group_emitters: false, key: VALID_KEY });
});

test('approve dialog shows a wrong-key error on HTTP 400', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/approve'))
      return Promise.resolve(jsonResponse('nope', 400));
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([PENDING]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /approve/i }));
  const dialog = screen.getByRole('dialog');
  // Valid-shaped key (so Confirm enables), but the backend rejects it 400.
  fireEvent.change(within(dialog).getByLabelText(/sensor encryption key/i), { target: { value: VALID_KEY } });
  fireEvent.click(within(dialog).getByRole('button', { name: /confirm/i }));
  await waitFor(() => expect(within(dialog).getByRole('alert')).toHaveTextContent(/doesn't match this sensor's fingerprint/i));
});

test('Allow new Sensors opens a counting-down window and disables the button', async () => {
  vi.useFakeTimers();
  try {
    vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (init?.method === 'POST' && url.includes('/allow-sensors'))
        return Promise.resolve(jsonResponse({ remaining_secs: 3 }));
      if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([]));
      if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([
        { id: 'dsX', created_at: '', kind: 'sensor', mode: 'listener', interface: null, status: 'running', config: { bind_ip: '0.0.0.0', bind_port: 9000 }, last_error: null, desired_state: 'running', last_ok_at: null },
      ]));
      return Promise.reject(new Error(url));
    }));
    render(<Sensors />, { wrapper });
    const btn = await vi.waitFor(() => {
      const b = screen.getByRole('button', { name: /allow new sensors/i });
      expect(b).not.toBeDisabled();
      return b;
    });
    fireEvent.click(btn);
    await vi.waitFor(() => expect(screen.getByText(/sensor enrollment window open for 3s/i)).toBeInTheDocument());
    expect(btn).toBeDisabled();
    // Advance past the window; the countdown clears and the button re-enables.
    await vi.advanceTimersByTimeAsync(3000);
    expect(screen.queryByText(/sensor enrollment window open/i)).not.toBeInTheDocument();
    expect(btn).not.toBeDisabled();
  } finally {
    vi.useRealTimers();
  }
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

const APPROVED: Sensor = {
  id: 's2', data_source_id: 'ds1', sensor_id: 'backlot', fingerprint: 'AA-BB-CC-DD',
  status: 'approved', auto_group_emitters: true, source_ip: '9.9.9.9',
  approved_at: '2026-07-20T00:00:00Z', last_seen_at: '2026-07-21T00:00:00Z', online: true,
  emissions_24h: 5,
};

test('registered sensor shows online + real 24h emission count + rotate reveals a one-time key', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/rotate'))
      return Promise.resolve(jsonResponse({ key: 'NEWKEYBASE64', fingerprint: 'EE-FF-00-11' }));
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  expect(await screen.findByText('backlot')).toBeInTheDocument();
  expect(screen.getByTestId('sensor-online-backlot')).toBeInTheDocument();
  expect(screen.getByText('5 emissions/24h')).toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: /rotate/i }));
  await waitFor(() => expect(screen.getByText('NEWKEYBASE64')).toBeInTheDocument());
  expect(screen.getByText(/shown once/i)).toBeInTheDocument();
});

test('a registered sensor shows the address it is reporting from', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  expect(await screen.findByTestId('sensor-ip-backlot')).toHaveTextContent('9.9.9.9');
});

test('a registered sensor with no address on file says so rather than rendering blank', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url.includes('/api/sensors'))
      return Promise.resolve(jsonResponse([{ ...APPROVED, source_ip: null }]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  expect(await screen.findByTestId('sensor-ip-backlot')).toHaveTextContent('unknown IP');
});

test('Allow new Sensors is disabled when no running sensor datasource exists', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([])); // none
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  const btn = await screen.findByRole('button', { name: /allow new sensors/i });
  expect(btn).toBeDisabled();
});

test('revoke asks for confirmation and posts to the revoke endpoint when confirmed', async () => {
  const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
  const calls: string[] = [];
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/revoke')) { calls.push(url); return Promise.resolve(jsonResponse({ ...APPROVED, status: 'revoked' })); }
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /revoke/i }));
  expect(confirmSpy).toHaveBeenCalledWith('Revoke backlot? The sensor must re-enroll to reconnect.');
  await waitFor(() => expect(calls.some((u) => u.includes('/revoke'))).toBe(true));
  confirmSpy.mockRestore();
});

test('revoke does nothing when confirmation is declined', async () => {
  const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const calls: string[] = [];
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST') { calls.push(url); return Promise.resolve(jsonResponse({ ...APPROVED, status: 'revoked' })); }
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /revoke/i }));
  expect(confirmSpy).toHaveBeenCalled();
  expect(calls.some((u) => u.includes('/revoke'))).toBe(false);
  confirmSpy.mockRestore();
});

test('surfaces a page-level alert when revoke fails', async () => {
  const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/revoke')) return Promise.resolve(jsonResponse({}, 500));
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /revoke/i }));
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/action failed/i));
  confirmSpy.mockRestore();
});

test('surfaces a page-level alert when rotate fails', async () => {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/rotate')) return Promise.resolve(jsonResponse({}, 500));
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([APPROVED]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  fireEvent.click(await screen.findByRole('button', { name: /rotate/i }));
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/action failed/i));
});

test('Allow new Sensors posts to the running sensor datasource', async () => {
  const calls: string[] = [];
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (init?.method === 'POST' && url.includes('/allow-sensors')) { calls.push(url); return Promise.resolve(jsonResponse({ remaining_secs: 900 })); }
    if (url.includes('/api/sensors')) return Promise.resolve(jsonResponse([]));
    if (url.includes('/api/data-sources')) return Promise.resolve(jsonResponse([
      { id: 'dsX', created_at: '', kind: 'sensor', mode: 'listener', interface: null, status: 'running', config: { bind_ip: '0.0.0.0', bind_port: 9000 }, last_error: null, desired_state: 'running', last_ok_at: null },
    ]));
    return Promise.reject(new Error(url));
  }));
  render(<Sensors />, { wrapper });
  const btn = await screen.findByRole('button', { name: /allow new sensors/i });
  await waitFor(() => expect(btn).not.toBeDisabled());
  fireEvent.click(btn);
  await waitFor(() => expect(calls.some((u) => u.includes('/api/data-sources/dsX/allow-sensors'))).toBe(true));
});

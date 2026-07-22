import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Settings from './Settings';
import { jsonResponse } from '../test-utils/fetchMocks';

afterEach(() => vi.unstubAllGlobals());
function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

function mock(config: unknown, calls: Array<{ url: string; body: unknown }>) {
  vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if ((init?.method ?? 'GET') === 'PATCH') { calls.push({ url, body: JSON.parse(init!.body as string) }); return Promise.resolve(jsonResponse(config)); }
    if (url.includes('/api/config')) return Promise.resolve(jsonResponse(config));
    return Promise.reject(new Error(url));
  }));
}

test('sensor node: save omits key when blank, includes host', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  mock({ role: 'sensor', node_sensor_id: 'frontgate', sensor: { host: 'base', port: 9000, cache_ttl_secs: 3600 } }, calls);
  render(<Settings />, { wrapper });
  await waitFor(() => expect((screen.getByLabelText(/standalone host/i) as HTMLInputElement).value).toBe('base'));
  fireEvent.change(screen.getByLabelText(/standalone host/i), { target: { value: 'base2' } });
  fireEvent.click(screen.getByRole('button', { name: /save/i }));
  await waitFor(() => expect(calls.length).toBe(1));
  const body = calls[0].body as { sensor: { host: string; key?: string } };
  expect(body.sensor.host).toBe('base2');
  expect(body.sensor.key).toBeUndefined(); // blank key omitted
});

test('switching a standalone node to sensor role requires a key', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  mock({ role: 'standalone', node_sensor_id: 'local', sensor: null }, calls);
  render(<Settings />, { wrapper });
  await waitFor(() => expect((screen.getByLabelText(/node sensor id/i) as HTMLInputElement).value).toBe('local'));

  fireEvent.click(screen.getByRole('radio', { name: /sensor/i }));
  fireEvent.change(screen.getByLabelText(/standalone host/i), { target: { value: 'base' } });
  fireEvent.change(screen.getByLabelText(/port/i), { target: { value: '9000' } });
  fireEvent.click(screen.getByRole('button', { name: /save/i }));

  expect(await screen.findByRole('alert')).toHaveTextContent(/key is required/i);
  expect(calls.length).toBe(0);

  const validKey = btoa('A'.repeat(32)); // 32 bytes, base64-encoded
  fireEvent.change(screen.getByLabelText(/encryption key/i), { target: { value: validKey } });
  fireEvent.click(screen.getByRole('button', { name: /save/i }));
  await waitFor(() => expect(calls.length).toBe(1));
  const body = calls[0].body as { sensor: { key?: string } };
  expect(body.sensor.key).toBe(validKey);
});

test('rejects a malformed (non-32-byte) encryption key', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  mock({ role: 'sensor', node_sensor_id: 'frontgate', sensor: { host: 'base', port: 9000, cache_ttl_secs: 3600 } }, calls);
  render(<Settings />, { wrapper });
  await waitFor(() => expect((screen.getByLabelText(/standalone host/i) as HTMLInputElement).value).toBe('base'));
  fireEvent.change(screen.getByLabelText(/encryption key/i), { target: { value: 'not-32-bytes' } });
  fireEvent.click(screen.getByRole('button', { name: /save/i }));
  expect(await screen.findByRole('alert')).toHaveTextContent(/32 bytes/i);
  expect(calls.length).toBe(0);
});

test('Generate fills the encryption key with a valid key', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  mock({ role: 'sensor', node_sensor_id: 'frontgate', sensor: { host: 'base', port: 9000, cache_ttl_secs: 3600 } }, calls);
  render(<Settings />, { wrapper });
  const keyField = await waitFor(() => screen.getByLabelText(/encryption key/i) as HTMLInputElement);
  expect(keyField.value).toBe('');
  fireEvent.click(screen.getByRole('button', { name: /generate/i }));
  expect(keyField.value.length).toBeGreaterThan(0);
  fireEvent.click(screen.getByRole('button', { name: /save/i }));
  await waitFor(() => expect(calls.length).toBe(1));
});

test('rejects a node id with a space', async () => {
  const calls: Array<{ url: string; body: unknown }> = [];
  mock({ role: 'standalone', node_sensor_id: 'local', sensor: null }, calls);
  render(<Settings />, { wrapper });
  await waitFor(() => expect((screen.getByLabelText(/node sensor id/i) as HTMLInputElement).value).toBe('local'));
  fireEvent.change(screen.getByLabelText(/node sensor id/i), { target: { value: 'bad id' } });
  fireEvent.click(screen.getByRole('button', { name: /save/i }));
  expect(await screen.findByRole('alert')).toBeInTheDocument();
  expect(calls.length).toBe(0);
});

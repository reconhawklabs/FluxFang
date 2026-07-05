import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Alerts from './Alerts';
import { jsonResponse } from '../test-utils/fetchMocks';
import type { AlertMethod } from '../api/alertMethods';
import type { AlertRule } from '../api/alertRules';

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

/** Method+pathname-aware fetch mock — same convention as
 * `DataSources.test.tsx`'s `mockMethodRoutes`. */
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

const EMAIL_METHOD: AlertMethod = {
  id: 'method-1',
  name: 'Ops email',
  type: 'email',
  enabled: true,
  created_at: '2026-07-01T00:00:00Z',
  // The safe GET projection never includes username/password — see
  // `fluxfang-api::alert_methods`'s `safe_config` allowlist.
  config: { host: 'smtp.example.com', port: 587, from: 'alerts@example.com', to: 'ops@example.com', tls: true },
};

const RULE_1: AlertRule = {
  id: 'rule-1',
  name: 'Bob detected',
  enabled: true,
  target_type: 'entity',
  target_id: 'entity-1',
  trigger: { on: 'detected' },
  method_ids: ['method-1'],
  created_at: '2026-07-01T00:00:00Z',
};

test('adding an email method reveals email fields and POSTs the config, without ever rendering a submitted secret', async () => {
  const created: AlertMethod = {
    id: 'method-new',
    name: 'New email',
    type: 'email',
    enabled: true,
    created_at: '2026-07-05T00:00:00Z',
    config: { host: 'smtp.example.com', port: 587, from: 'a@example.com', to: 'b@example.com', tls: true },
  };
  const fetchMock = mockMethodRoutes({
    'GET /api/alert-methods': () => [],
    'GET /api/alert-rules': () => [],
    'POST /api/alert-methods': () => created,
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Alerts />, { wrapper });
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());

  fireEvent.click(screen.getByRole('button', { name: /add method/i }));

  // email is the default type — its fields should already be visible.
  expect(screen.getByLabelText(/^host$/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/^port$/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/^password$/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/^from$/i)).toBeInTheDocument();
  expect(screen.getByLabelText(/^to$/i)).toBeInTheDocument();

  const secretPassword = 'sUpErSecret_p4ssw0rd!';
  fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: 'New email' } });
  fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: 'smtp.example.com' } });
  fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: '587' } });
  fireEvent.change(screen.getByLabelText(/username/i), { target: { value: 'alerts@example.com' } });
  fireEvent.change(screen.getByLabelText(/^password$/i), { target: { value: secretPassword } });
  fireEvent.change(screen.getByLabelText(/^from$/i), { target: { value: 'a@example.com' } });
  fireEvent.change(screen.getByLabelText(/^to$/i), { target: { value: 'b@example.com' } });

  fireEvent.click(screen.getByRole('button', { name: /^add$/i }));

  await waitFor(() => expect(screen.queryByLabelText(/^host$/i)).not.toBeInTheDocument());

  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === '/api/alert-methods' && init?.method === 'POST',
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({
    name: 'New email',
    type: 'email',
    enabled: true,
    config: {
      host: 'smtp.example.com',
      port: 587,
      username: 'alerts@example.com',
      password: secretPassword,
      from: 'a@example.com',
      to: 'b@example.com',
      tls: true,
    },
  });

  // The submitted password is never rendered back on the page anywhere.
  expect(screen.queryByText(secretPassword)).not.toBeInTheDocument();
});

test('the GET list response (which has no secret field) renders without exposing a password', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/alert-methods': () => [EMAIL_METHOD],
    'GET /api/alert-rules': () => [],
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Alerts />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('alert-method-row-method-1')).toBeInTheDocument());

  const row = screen.getByTestId('alert-method-row-method-1');
  expect(within(row).getByText('Ops email')).toBeInTheDocument();
  expect(within(row).getByText('email')).toBeInTheDocument();
  // No password/username is anywhere in the rendered row (the DTO simply
  // never carries one, but this guards against ever putting one there).
  expect(row.textContent).not.toMatch(/password/i);
});

test('"Send test" calls POST /api/alert-methods/:id/test and renders Delivered', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/alert-methods': () => [EMAIL_METHOD],
    'GET /api/alert-rules': () => [],
    'POST /api/alert-methods/method-1/test': () => ({ status: 'delivered' }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Alerts />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('alert-method-row-method-1')).toBeInTheDocument());

  const row = screen.getByTestId('alert-method-row-method-1');
  fireEvent.click(within(row).getByRole('button', { name: /send test/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/alert-methods/method-1/test',
      expect.objectContaining({ method: 'POST' }),
    ),
  );
  await waitFor(() => expect(within(row).getByText(/delivered/i)).toBeInTheDocument());
});

test('"Send test" renders Failed + reason when delivery fails', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/alert-methods': () => [EMAIL_METHOD],
    'GET /api/alert-rules': () => [],
    'POST /api/alert-methods/method-1/test': () => ({ status: 'failed', reason: 'connection refused' }),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Alerts />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('alert-method-row-method-1')).toBeInTheDocument());

  const row = screen.getByTestId('alert-method-row-method-1');
  fireEvent.click(within(row).getByRole('button', { name: /send test/i }));

  await waitFor(() => expect(within(row).getByText(/failed/i)).toBeInTheDocument());
  expect(within(row).getByText(/connection refused/i)).toBeInTheDocument();
});

test('renders the alert rules list read-only with target/trigger/method count', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/alert-methods': () => [EMAIL_METHOD],
    'GET /api/alert-rules': () => [RULE_1],
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Alerts />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('alert-rule-row-rule-1')).toBeInTheDocument());

  const row = screen.getByTestId('alert-rule-row-rule-1');
  expect(within(row).getByText('Bob detected')).toBeInTheDocument();
  expect(within(row).getByText('entity')).toBeInTheDocument();
  expect(within(row).getByText('detected')).toBeInTheDocument();
  expect(within(row).getByText('1')).toBeInTheDocument(); // method_ids.length
});

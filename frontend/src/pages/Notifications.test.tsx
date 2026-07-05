import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Notifications from './Notifications';
import { jsonResponse } from '../test-utils/fetchMocks';
import { notificationStore } from '../store/notificationStore';
import type { Notification, NotificationsPage } from '../api/notifications';

afterEach(() => {
  vi.unstubAllGlobals();
});

beforeEach(() => {
  notificationStore.reset();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

/** Method+pathname-aware fetch mock — same convention as
 * `DataSources.test.tsx`'s `mockMethodRoutes`. Query strings are ignored
 * for routing (both GETs here hit the same `/api/notifications` path with
 * different `unread_only`/`limit`/`offset` params); a handler that cares
 * can still inspect them off the passed `URL`. */
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

const UNREAD_NOTIFICATION: Notification = {
  id: 'notif-1',
  alert_rule_id: 'rule-1',
  alert_method_id: 'method-1',
  fired_at: '2026-07-05T10:00:00Z',
  payload: { title: 'Bob detected', body: 'Bob was detected at Home' },
  delivery_status: 'sent',
  read_at: null,
};

const READ_NOTIFICATION: Notification = {
  id: 'notif-2',
  alert_rule_id: 'rule-1',
  alert_method_id: 'method-1',
  fired_at: '2026-07-04T09:00:00Z',
  payload: { title: 'Alice detected', body: 'Alice was detected at Work' },
  delivery_status: 'sent',
  read_at: '2026-07-04T09:05:00Z',
};

function page(items: Notification[], unreadCount: number, total?: number): NotificationsPage {
  return { items, total: total ?? items.length, unread_count: unreadCount };
}

test('renders notifications with title/body/fired_at/status and read vs unread state', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/notifications': () => page([UNREAD_NOTIFICATION, READ_NOTIFICATION], 1),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Notifications />, { wrapper });

  await waitFor(() => expect(screen.getByTestId('notification-row-notif-1')).toBeInTheDocument());

  const unreadRow = screen.getByTestId('notification-row-notif-1');
  expect(within(unreadRow).getByText('Bob detected')).toBeInTheDocument();
  expect(within(unreadRow).getByText('Bob was detected at Home')).toBeInTheDocument();
  expect(within(unreadRow).getByText(new Date('2026-07-05T10:00:00Z').toLocaleString())).toBeInTheDocument();
  expect(within(unreadRow).getByTestId('notification-delivery-status')).toHaveTextContent(/sent/i);
  expect(within(unreadRow).getByRole('button', { name: /mark read/i })).toBeInTheDocument();
  expect(within(unreadRow).getByTestId('notification-unread-dot-notif-1')).toBeInTheDocument();

  const readRow = screen.getByTestId('notification-row-notif-2');
  expect(within(readRow).queryByRole('button', { name: /mark read/i })).not.toBeInTheDocument();
  expect(within(readRow).getByText(/^read$/i)).toBeInTheDocument();
  expect(within(readRow).queryByTestId('notification-unread-dot-notif-2')).not.toBeInTheDocument();

  // The header-level unread count is shown too.
  expect(screen.getByTestId('notifications-unread-count')).toHaveTextContent('1');
});

test('"Mark read" POSTs /api/notifications/:id/read and the item flips to read', async () => {
  // Mutable backing "row" so the mocked GET (re-run after the mutation
  // invalidates `queryKeys.notifications`) reflects the mark-read that
  // just happened — same as a real backend would.
  let current: Notification = { ...UNREAD_NOTIFICATION };
  const fetchMock = mockMethodRoutes({
    'GET /api/notifications': () => page([current], current.read_at === null ? 1 : 0),
    'POST /api/notifications/notif-1/read': () => {
      current = { ...current, read_at: '2026-07-05T12:00:00Z' };
      return current;
    },
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Notifications />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('notification-row-notif-1')).toBeInTheDocument());

  const row = screen.getByTestId('notification-row-notif-1');
  expect(within(row).getByRole('button', { name: /mark read/i })).toBeInTheDocument();
  fireEvent.click(within(row).getByRole('button', { name: /mark read/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/notifications/notif-1/read',
      expect.objectContaining({ method: 'POST' }),
    ),
  );

  await waitFor(() => expect(within(row).getByText(/^read$/i)).toBeInTheDocument());
  expect(within(row).queryByRole('button', { name: /mark read/i })).not.toBeInTheDocument();

  // The badge is also reconciled to the now-lower server unread_count.
  await waitFor(() => expect(notificationStore.getUnread()).toBe(0));
});

test('the header unread badge (notificationStore) reflects the server unread_count on load', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/notifications': () => page([UNREAD_NOTIFICATION], 3, 5),
  });
  vi.stubGlobal('fetch', fetchMock);

  expect(notificationStore.getUnread()).toBe(0);

  render(<Notifications />, { wrapper });

  await waitFor(() => expect(notificationStore.getUnread()).toBe(3));
});

test('"unread only" toggle refetches with unread_only=true', async () => {
  const fetchMock = mockMethodRoutes({
    'GET /api/notifications': (url) =>
      url.searchParams.get('unread_only') === 'true'
        ? page([UNREAD_NOTIFICATION], 1)
        : page([UNREAD_NOTIFICATION, READ_NOTIFICATION], 1),
  });
  vi.stubGlobal('fetch', fetchMock);

  render(<Notifications />, { wrapper });
  await waitFor(() => expect(screen.getByTestId('notification-row-notif-2')).toBeInTheDocument());

  fireEvent.click(screen.getByLabelText(/unread only/i));

  await waitFor(() => expect(screen.getByTestId('notification-row-notif-1')).toBeInTheDocument());
  expect(screen.queryByTestId('notification-row-notif-2')).not.toBeInTheDocument();
});

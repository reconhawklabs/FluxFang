import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import { useLiveEvents } from './useLiveEvents';
import { notificationStore } from '../store/notificationStore';

/** Minimal fake standing in for the browser `WebSocket`, driven manually by
 * each test via its `onmessage`/`onclose` handlers. */
class FakeSocket {
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  close = vi.fn();
}

function renderWithClient(queryClient: QueryClient, wsFactory: () => WebSocket) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return renderHook(() => useLiveEvents({ enabled: true, wsFactory }), { wrapper });
}

beforeEach(() => {
  notificationStore.reset();
});

test('an "emission" WS frame invalidates the emissions (and dashboard) query', () => {
  const queryClient = new QueryClient();
  const invalidateSpy = vi.spyOn(queryClient, 'invalidateQueries');

  let socket: FakeSocket | undefined;
  const wsFactory = vi.fn(() => {
    socket = new FakeSocket();
    return socket as unknown as WebSocket;
  });

  renderWithClient(queryClient, wsFactory);

  socket!.onmessage?.({ data: JSON.stringify({ type: 'emission', data: { id: '1' } }) });

  expect(invalidateSpy).toHaveBeenCalledWith(expect.objectContaining({ queryKey: ['emissions'] }));
  expect(invalidateSpy).toHaveBeenCalledWith(expect.objectContaining({ queryKey: ['dashboard'] }));
});

test('a "notification" WS frame invalidates notifications and bumps the unread badge', () => {
  const queryClient = new QueryClient();
  const invalidateSpy = vi.spyOn(queryClient, 'invalidateQueries');

  let socket: FakeSocket | undefined;
  const wsFactory = vi.fn(() => {
    socket = new FakeSocket();
    return socket as unknown as WebSocket;
  });

  renderWithClient(queryClient, wsFactory);

  expect(notificationStore.getUnread()).toBe(0);

  socket!.onmessage?.({ data: JSON.stringify({ type: 'notification', data: { id: '1' } }) });

  expect(invalidateSpy).toHaveBeenCalledWith(expect.objectContaining({ queryKey: ['notifications'] }));
  expect(notificationStore.getUnread()).toBe(1);
});

test('a "lagged" WS frame invalidates every query', () => {
  const queryClient = new QueryClient();
  const invalidateSpy = vi.spyOn(queryClient, 'invalidateQueries');

  let socket: FakeSocket | undefined;
  const wsFactory = vi.fn(() => {
    socket = new FakeSocket();
    return socket as unknown as WebSocket;
  });

  renderWithClient(queryClient, wsFactory);

  socket!.onmessage?.({ data: JSON.stringify({ type: 'lagged', dropped: 3 }) });

  expect(invalidateSpy).toHaveBeenCalledWith();
});

test('reconnects with backoff after the socket closes, while enabled', () => {
  vi.useFakeTimers();
  try {
    const queryClient = new QueryClient();
    const wsFactory = vi.fn(() => new FakeSocket() as unknown as WebSocket);

    renderWithClient(queryClient, wsFactory);
    expect(wsFactory).toHaveBeenCalledTimes(1);

    const firstSocket = wsFactory.mock.results[0]!.value as unknown as FakeSocket;
    firstSocket.onclose?.();

    vi.advanceTimersByTime(1000);
    expect(wsFactory).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
  }
});

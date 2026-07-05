// Subscribes to the `/ws` live-event stream while `enabled` (i.e. while the
// user is authenticated — see `App.tsx`) and, per frame:
//
// - `emission`   → invalidate `queryKeys.emissions` + `queryKeys.dashboard`
// - `notification` → invalidate `queryKeys.notifications` + bump the
//   unread-badge counter (`notificationStore`)
// - `lagged`     → invalidate everything (messages were dropped server-side
//   — see `crates/fluxfang-api/src/ws.rs`'s `WireOutcome::Send` lagged
//   case — so any of the above may be stale)
//
// See `src/api/queryKeys.ts` for the full key-invalidation convention later
// pages must follow.
//
// Reconnect: on `close` (including after an `error`, which this closes the
// socket to trigger), retries with capped exponential backoff
// (`reconnectDelayMs`). The socket is torn down and no reconnect is
// attempted once `enabled` goes false or the component unmounts.
import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { parseWsFrame, reconnectDelayMs, wsUrl } from '../api/ws';
import { queryKeys } from '../api/queryKeys';
import { notificationStore } from '../store/notificationStore';

export interface UseLiveEventsOptions {
  /** Only connect while true — pass `authed` from `useAuth()`. */
  enabled: boolean;
  /** Override for tests; defaults to the real `WebSocket` constructor. */
  wsFactory?: (url: string) => WebSocket;
}

const defaultWsFactory = (url: string): WebSocket => new WebSocket(url);

export function useLiveEvents({ enabled, wsFactory = defaultWsFactory }: UseLiveEventsOptions): void {
  const queryClient = useQueryClient();

  // Kept in a ref so a new `wsFactory` reference each render doesn't tear
  // down and reconnect the live socket — only `enabled` should do that.
  const wsFactoryRef = useRef(wsFactory);
  wsFactoryRef.current = wsFactory;

  useEffect(() => {
    if (!enabled) return;

    let stopped = false;
    let attempt = 0;
    let socket: WebSocket | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    const connect = (): void => {
      socket = wsFactoryRef.current(wsUrl());

      socket.onopen = () => {
        attempt = 0;
      };

      socket.onmessage = (event: MessageEvent) => {
        const frame = parseWsFrame(String(event.data));
        if (!frame) return;

        switch (frame.type) {
          case 'emission':
            queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
            queryClient.invalidateQueries({ queryKey: queryKeys.dashboard });
            break;
          case 'notification':
            queryClient.invalidateQueries({ queryKey: queryKeys.notifications });
            notificationStore.bumpUnread();
            break;
          case 'lagged':
            queryClient.invalidateQueries();
            break;
        }
      };

      socket.onclose = () => {
        if (stopped) return;
        const delay = reconnectDelayMs(attempt);
        attempt += 1;
        reconnectTimer = setTimeout(connect, delay);
      };

      socket.onerror = () => {
        socket?.close();
      };
    };

    connect();

    return () => {
      stopped = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      socket?.close();
    };
  }, [enabled, queryClient]);
}

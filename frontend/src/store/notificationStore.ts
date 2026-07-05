// Tiny external store for the AppShell's unread-notifications badge.
//
// This is a *live* counter — it bumps immediately when `useLiveEvents` sees
// a `{"type":"notification"}` WS frame, independent of (and faster than)
// the authoritative `unread_count` a later task's `GET /api/notifications`
// query will show. It's reset via `reset()`, which the Notifications page
// (Task 9.9) should call once the user has viewed/acknowledged the list.
//
// Implemented as a plain module-level pub/sub rather than a dependency
// (no zustand/redux in package.json) — `useUnreadCount` bridges it into
// React via `useSyncExternalStore`.
import { useSyncExternalStore } from 'react';

type Listener = () => void;

let unread = 0;
const listeners = new Set<Listener>();

function emit(): void {
  for (const listener of listeners) listener();
}

export const notificationStore = {
  bumpUnread(): void {
    unread += 1;
    emit();
  },
  reset(): void {
    unread = 0;
    emit();
  },
  getUnread(): number {
    return unread;
  },
  subscribe(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};

function getServerSnapshot(): number {
  return 0;
}

export function useUnreadCount(): number {
  return useSyncExternalStore(notificationStore.subscribe, notificationStore.getUnread, getServerSnapshot);
}

// Tiny external store for the AppShell's unread-notifications badge.
//
// This is a *live* counter — it bumps immediately when `useLiveEvents` sees
// a `{"type":"notification"}` WS frame, independent of (and faster than)
// the authoritative `unread_count` `GET /api/notifications` reports. Two
// ways it gets reconciled back to that authoritative value, both added by
// Task 9.9's Notifications page:
//
// - `setUnread(n)`: called with the server's `unread_count` whenever that
//   page's `GET /api/notifications` query (re)settles — on mount, on
//   refetch (e.g. a live WS `notification` frame invalidating
//   `queryKeys.notifications`), and after `POST .../:id/read` succeeds.
//   This is the general-purpose sync: the badge always ends up showing
//   exactly what the server thinks is unread, regardless of how many WS
//   frames did or didn't arrive in between.
// - `reset()`: a plain zero-out, kept for the case where the count is
//   known to be zero outright (e.g. tests, or a future "mark all read"
//   affordance) without needing a server round-trip's number in hand.
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
  /** Reconcile the badge to the server's authoritative `unread_count` (see
   * module doc comment). Clamped to >= 0 defensively — the server should
   * never report negative, but a badge can't sensibly show one either. */
  setUnread(count: number): void {
    unread = Math.max(0, count);
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

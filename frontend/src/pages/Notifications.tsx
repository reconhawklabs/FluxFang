// Task 9.9: the in-app notifications inbox — `GET /api/notifications`
// (paginated via `limit`/`offset`/`total`, filterable via `unread_only`),
// "Mark read" per item (`POST /api/notifications/:id/read`), and a header
// unread-badge reconciliation.
//
// Live updates: `useLiveEvents` (Task 9.1) already invalidates
// `queryKeys.notifications` on every `{"type":"notification"}` WS frame.
// This page's query key is `[...queryKeys.notifications, filters]` —
// `invalidateQueries({ queryKey: queryKeys.notifications })` matches by
// prefix (see `queryKeys.ts`'s module doc comment), so that invalidation
// reaches this filtered key too and a new notification shows up here
// without any bespoke WS handling in this file.
//
// Badge reconciliation: `notificationStore` (Task 9.1) bumps its counter
// live off the WS stream, which can drift from the truth — e.g. it starts
// at 0 on page load even if there were already unread notifications from a
// previous session, and it never decrements when the user reads something.
// This page is the fix: every time its `GET /api/notifications` query
// (re)settles (initial mount, a WS-triggered refetch, or after a
// mark-read mutation invalidates it), it calls the new
// `notificationStore.setUnread(unread_count)` with the server's
// authoritative count, snapping the badge to the truth regardless of how
// many WS frames did or didn't arrive in between.
import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { listNotifications, markNotificationRead } from '../api/notifications';
import { notificationStore } from '../store/notificationStore';

/** Page size for `limit` — this page's own pagination convention, not a
 * backend default (the backend's own `DEFAULT_LIMIT` of 50 only applies
 * when `limit` is omitted entirely; this page always sends one). */
const PAGE_SIZE = 20;

function formatTimestamp(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

const STATUS_BADGE_CLASSES: Record<string, string> = {
  sent: 'bg-green-500/20 text-green-400',
  failed: 'bg-red-500/20 text-red-400',
  pending: 'bg-amber-500/20 text-amber-400',
};

function DeliveryStatusBadge({ status }: { status: string }) {
  return (
    <span
      data-testid="notification-delivery-status"
      className={`inline-block rounded px-2 py-0.5 text-xs font-medium capitalize ${STATUS_BADGE_CLASSES[status] ?? 'bg-slate-700 text-slate-300'}`}
    >
      {status}
    </span>
  );
}

export default function Notifications() {
  const queryClient = useQueryClient();
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [offset, setOffset] = useState(0);

  const filters = { unread_only: unreadOnly, limit: PAGE_SIZE, offset };
  const notificationsQuery = useQuery({
    queryKey: [...queryKeys.notifications, filters],
    queryFn: () => listNotifications(filters),
  });

  const unreadCount = notificationsQuery.data?.unread_count;

  // Reconcile the header badge to the server's authoritative unread_count
  // every time it changes (see module doc comment).
  useEffect(() => {
    if (unreadCount !== undefined) {
      notificationStore.setUnread(unreadCount);
    }
  }, [unreadCount]);

  function invalidate(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.notifications });
  }

  const markReadMutation = useMutation({
    mutationFn: (id: string) => markNotificationRead(id),
    onSuccess: invalidate,
  });

  const markAllReadMutation = useMutation({
    mutationFn: (ids: string[]) => Promise.all(ids.map((id) => markNotificationRead(id))),
    onSuccess: invalidate,
  });

  function handleToggleUnreadOnly(next: boolean): void {
    setUnreadOnly(next);
    setOffset(0);
  }

  const items = notificationsQuery.data?.items ?? [];
  const total = notificationsQuery.data?.total ?? 0;
  const unreadIdsOnPage = items.filter((n) => n.read_at === null).map((n) => n.id);
  const hasPrev = offset > 0;
  const hasNext = offset + items.length < total;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">
          Notifications
          {unreadCount !== undefined && unreadCount > 0 && (
            <span data-testid="notifications-unread-count" className="ml-2 text-sm font-normal text-amber-400">
              {unreadCount} unread
            </span>
          )}
        </h1>
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={unreadOnly}
              onChange={(event) => handleToggleUnreadOnly(event.target.checked)}
            />
            Unread only
          </label>
          <button
            type="button"
            disabled={unreadIdsOnPage.length === 0 || markAllReadMutation.isPending}
            onClick={() => markAllReadMutation.mutate(unreadIdsOnPage)}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Mark all read
          </button>
        </div>
      </div>

      {notificationsQuery.isLoading && <p className="text-sm text-slate-500">Loading notifications…</p>}
      {notificationsQuery.isError && <p className="text-sm text-red-400">Failed to load notifications.</p>}
      {notificationsQuery.data && items.length === 0 && (
        <p className="text-sm text-slate-500">{unreadOnly ? 'No unread notifications.' : 'No notifications yet.'}</p>
      )}

      {items.length > 0 && (
        <ul className="space-y-2">
          {items.map((notification) => {
            const isUnread = notification.read_at === null;
            return (
              <li
                key={notification.id}
                data-testid={`notification-row-${notification.id}`}
                className={`rounded border p-3 ${
                  isUnread ? 'border-amber-500/40 bg-slate-900/60' : 'border-slate-800 bg-slate-950/40'
                }`}
              >
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <p className="text-sm font-medium text-slate-100">
                      {notification.payload.title ?? 'Notification'}
                      {isUnread && (
                        <span
                          data-testid={`notification-unread-dot-${notification.id}`}
                          className="ml-2 inline-block h-2 w-2 rounded-full bg-amber-500 align-middle"
                        />
                      )}
                    </p>
                    {notification.payload.body && (
                      <p className="mt-1 text-sm text-slate-400">{notification.payload.body}</p>
                    )}
                    <p className="mt-1 text-xs text-slate-500">{formatTimestamp(notification.fired_at)}</p>
                  </div>
                  <div className="flex flex-col items-end gap-2">
                    <DeliveryStatusBadge status={notification.delivery_status} />
                    {isUnread ? (
                      <button
                        type="button"
                        disabled={markReadMutation.isPending && markReadMutation.variables === notification.id}
                        onClick={() => markReadMutation.mutate(notification.id)}
                        className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        Mark read
                      </button>
                    ) : (
                      <span className="text-xs text-slate-500">Read</span>
                    )}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      {total > 0 && (
        <div className="flex items-center justify-between text-sm text-slate-400">
          <span>
            {offset + 1}–{Math.min(offset + items.length, total)} of {total}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={!hasPrev}
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
              className="rounded border border-slate-700 px-3 py-1.5 text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
            >
              Previous
            </button>
            <button
              type="button"
              disabled={!hasNext}
              onClick={() => setOffset(offset + PAGE_SIZE)}
              className="rounded border border-slate-700 px-3 py-1.5 text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

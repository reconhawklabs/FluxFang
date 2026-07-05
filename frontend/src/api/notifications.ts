// `GET /api/notifications` + `POST /api/notifications/:id/read` (Task 6.6
// backend, `fluxfang-api::notifications`). This task's (9.9) Notifications
// page is the only consumer — the in-app inbox lists `items`, paginates via
// `limit`/`offset`/`total`, and syncs the AppShell header badge
// (`notificationStore`, Task 9.1) to the authoritative `unread_count` this
// endpoint reports (see that store's doc comment for why this is needed
// alongside the WS-driven `bumpUnread()`).
import { get, post } from './client';

/** Mirrors `fluxfang-api::notifications::NotificationDto`. `payload` is
 * whatever `NotificationPayload` (`title`/`body`/`context`) the alert was
 * fired/tested with, serialized as JSON — modeled loosely here since this
 * page only ever reads `title`/`body` off it for display. */
export interface Notification {
  id: string;
  alert_rule_id: string | null;
  alert_method_id: string | null;
  fired_at: string;
  payload: { title?: string; body?: string; [key: string]: unknown };
  delivery_status: string;
  read_at: string | null;
}

/** `GET /api/notifications` query params — mirrors the backend's
 * `ListNotificationsQuery`. All optional; the backend defaults/clamps
 * `limit`/`offset` and treats a missing `unread_only` as `false`. */
export interface ListNotificationsParams {
  unread_only?: boolean;
  limit?: number;
  offset?: number;
}

/** `GET /api/notifications` response — mirrors `NotificationsPageDto`.
 * `unread_count` is a nav-bar-badge-style total independent of
 * `unread_only`/pagination, not just `items.length`. */
export interface NotificationsPage {
  items: Notification[];
  total: number;
  unread_count: number;
}

export function listNotifications(params: ListNotificationsParams = {}): Promise<NotificationsPage> {
  const query = new URLSearchParams();
  if (params.unread_only) query.set('unread_only', 'true');
  if (params.limit !== undefined) query.set('limit', String(params.limit));
  if (params.offset !== undefined) query.set('offset', String(params.offset));
  const qs = query.toString();
  return get<NotificationsPage>(`/api/notifications${qs.length > 0 ? `?${qs}` : ''}`);
}

export function markNotificationRead(id: string): Promise<Notification> {
  return post<Notification>(`/api/notifications/${id}/read`);
}

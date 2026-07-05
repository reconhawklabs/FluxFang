// Pure helpers for the `/ws` live-event stream (kept free of React so
// `useLiveEvents` can unit-test the reconnect/parsing logic without a real
// socket, and so it's obvious where the "wire contract" with the backend
// (`crates/fluxfang-api/src/ws.rs`) lives).

/** Same-origin `ws(s)://…/ws` URL — the browser sends the session cookie
 * automatically for a same-origin WebSocket handshake, same as any other
 * same-origin request, so no token needs to be threaded through here. */
export function wsUrl(path = '/ws'): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${window.location.host}${path}`;
}

/** Mirrors `crates/fluxfang-api/src/ws.rs`'s wire shape: every frame is
 * `{"type": "...", ...}` — `emission`/`notification` carry `data`, `lagged`
 * carries `dropped`. */
export type WsFrame =
  | { type: 'emission'; data: unknown }
  | { type: 'notification'; data: unknown }
  | { type: 'lagged'; dropped: number };

/** Parse one WS text frame, or `null` if it isn't a recognized shape
 * (malformed JSON, or missing/unknown `type`) — callers should silently
 * ignore `null` rather than throw, since one bad frame must not kill the
 * connection. */
export function parseWsFrame(raw: string): WsFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }

  if (
    value !== null &&
    typeof value === 'object' &&
    'type' in value &&
    typeof (value as { type: unknown }).type === 'string'
  ) {
    const type = (value as { type: string }).type;
    if (type === 'emission' || type === 'notification' || type === 'lagged') {
      return value as WsFrame;
    }
  }
  return null;
}

/** Exponential backoff (capped) for WS reconnect attempts, `attempt` being
 * the zero-indexed count of consecutive failed connections so far. */
export function reconnectDelayMs(attempt: number, baseMs = 1000, maxMs = 30000): number {
  return Math.min(baseMs * 2 ** attempt, maxMs);
}

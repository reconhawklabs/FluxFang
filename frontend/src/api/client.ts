// Typed fetch wrapper for the FluxFang API. Every call sends
// `credentials: 'include'` (session-cookie auth, see `useAuth`/backend
// `crates/fluxfang-api::middleware::require_auth`) and throws `ApiError` on
// any non-2xx response, carrying the HTTP status + a best-effort message.
//
// This module intentionally stays minimal: the generic `get`/`post`/`patch`/
// `del` helpers plus the auth endpoints needed by Task 2.3 (Setup/Login/
// useAuth). Resource-specific methods (data sources, emissions, emitters,
// ...) belong to whichever later task builds that page — add them here as
// small named functions/objects following the same pattern, not by growing
// this file's scope now (YAGNI).

const JSON_HEADERS = { 'Content-Type': 'application/json' };

/** Thrown by every helper below on a non-2xx response. */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

/**
 * Best-effort human-readable message for a failed response. Some endpoints
 * (the original auth routes) send a bare status with no body, in which case
 * this falls back to the status text; others send a JSON `{"message": ...}"`
 * body. Several later endpoints (e.g. the emissions/emitters routes' `400`s
 * — see `fluxfang-api::emissions`/`emitters`'s `ApiError::into_response`,
 * which return `(StatusCode::BAD_REQUEST, msg).into_response()`) send the
 * validation message as a **plain-text** body instead of JSON, so this tries
 * JSON first and falls back to the raw text body (when non-empty) before
 * finally falling back to `statusText`.
 */
async function errorMessage(res: Response): Promise<string> {
  const cloned = res.clone();

  try {
    const body: unknown = await res.clone().json();
    if (
      body &&
      typeof body === 'object' &&
      'message' in body &&
      typeof (body as { message: unknown }).message === 'string'
    ) {
      return (body as { message: string }).message;
    }
  } catch {
    // Response body wasn't JSON (or was empty) — fall through.
  }

  try {
    const text = await cloned.text();
    if (text.length > 0) return text;
  } catch {
    // Body already consumed/unreadable — fall through to statusText.
  }

  return res.statusText || `Request failed with status ${res.status}`;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: 'include',
    ...init,
    headers: { ...JSON_HEADERS, ...(init?.headers ?? {}) },
  });

  if (!res.ok) {
    throw new ApiError(res.status, await errorMessage(res));
  }

  // Several endpoints (e.g. `/api/setup`, `/api/login`, `/api/logout`)
  // respond with a bare status code and no body.
  const text = await res.text();
  return (text.length > 0 ? JSON.parse(text) : undefined) as T;
}

export function get<T>(path: string): Promise<T> {
  return request<T>(path, { method: 'GET' });
}

export function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: 'POST',
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

export function patch<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: 'PATCH',
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

export function del<T>(path: string): Promise<T> {
  return request<T>(path, { method: 'DELETE' });
}

/** `GET /api/setup/status` response shape. */
export interface SetupStatus {
  needs_setup: boolean;
}

export type NodeRole = 'standalone' | 'sensor';

/** `GET /api/config` response shape (secret-free). */
export interface AppConfig {
  role: NodeRole;
  node_sensor_id: string;
  sensor?: { host: string; port: number; cache_ttl_secs: number } | null;
}

/** `PATCH /api/config` partial body — omitted fields keep their stored value;
 * an omitted `sensor.key` keeps the current key (the key is never returned). */
export interface ConfigPatch {
  node_sensor_id?: string;
  role?: NodeRole;
  sensor?: { host?: string; port?: number; key?: string; cache_ttl_secs?: number };
}

/** A sensor node's connection block, sent only when `role === 'sensor'`. */
export interface SensorSetup {
  host: string;
  port: number;
  key: string;
  cache_ttl_secs: number;
}

/** `POST /api/setup` request body. */
export interface SetupRequest {
  password: string;
  role: NodeRole;
  node_sensor_id: string;
  sensor?: SensorSetup;
}

/** `GET /api/sensor/status` response shape — a Sensor node's own forwarding
 * status: its cache depth/backlog and the standalone target it forwards to. */
export interface SensorStatus {
  role: NodeRole | null;
  node_sensor_id: string | null;
  cache: { total: number; undelivered: number };
  /** Emissions forwarded to the Standalone in the last hour. */
  delivered_last_hour: number;
  /** Live reachability of the Standalone listener; null if not a sensor.
   *  Reachability only — it says nothing about whether batches are landing,
   *  which is what `forwarding` is for. */
  connected: boolean | null;
  /** What the forwarding loop is actually achieving. */
  forwarding: ForwarderStatus;
  sensor: { host: string; port: number } | null;
}

/** `GET /api/sensor/status` -> `forwarding`. Distinguishes "the Standalone
 *  answers pings" from "our emissions are getting there", which the old
 *  reachability-only signal could not: a sensor whose every batch was failing
 *  still displayed as connected while the Standalone listed it as down. */
export interface ForwarderStatus {
  state: 'paused' | 'enrolling' | 'forwarding';
  last_contact_at: string | null;
  last_delivery_at: string | null;
  delivered_since_start: number;
  /** Why the last cycle failed. Cleared on success, so non-null means
   *  forwarding is broken right now. */
  last_error: string | null;
}

/** `POST /api/sensor/request-approval` response.
 *
 * The operator-driven alternative to waiting on the forwarder's invisible
 * ~30s retry. `detail` is always safe to show verbatim: the backend fills it
 * from the same forwarding-health record the dashboard reads, so the button
 * and the status tile never disagree. */
export interface ApprovalRequestResult {
  status: 'approved' | 'pending' | 'not_configured';
  detail: string;
  sensor_id?: string;
  fingerprint?: string;
}

/** `GET /api/cached-emissions` row shape — an emission captured locally by a
 * Sensor node and cached pending delivery to its standalone target. */
export interface CachedEmission {
  id: string;
  created_at: string;
  kind: string;
  signal_strength: number | null;
  lat: number | null;
  lon: number | null;
  observed_at: string;
  payload: unknown;
  data_source_id: string | null;
  delivered: boolean;
}

/** Auth endpoints — the only resource methods this foundation task owns. */
export const api = {
  setupStatus: (): Promise<SetupStatus> => get<SetupStatus>('/api/setup/status'),
  setup: (req: SetupRequest): Promise<void> => post<void>('/api/setup', req),
  config: (): Promise<AppConfig> => get<AppConfig>('/api/config'),
  updateConfig: (patchBody: ConfigPatch): Promise<AppConfig> => patch<AppConfig>('/api/config', patchBody),
  login: (password: string): Promise<void> => post<void>('/api/login', { password }),
  logout: (): Promise<void> => post<void>('/api/logout'),
  sensorStatus: (): Promise<SensorStatus> => get<SensorStatus>('/api/sensor/status'),
  requestApproval: (): Promise<ApprovalRequestResult> =>
    post<ApprovalRequestResult>('/api/sensor/request-approval'),
  cachedEmissions: (limit = 100): Promise<CachedEmission[]> =>
    get<CachedEmission[]>(`/api/cached-emissions?limit=${limit}`),
};

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

/** Auth endpoints — the only resource methods this foundation task owns. */
export const api = {
  setupStatus: (): Promise<SetupStatus> => get<SetupStatus>('/api/setup/status'),
  setup: (password: string): Promise<void> => post<void>('/api/setup', { password }),
  login: (password: string): Promise<void> => post<void>('/api/login', { password }),
  logout: (): Promise<void> => post<void>('/api/logout'),
};

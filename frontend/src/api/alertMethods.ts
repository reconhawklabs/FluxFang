// `GET/POST/PATCH/DELETE /api/alert-methods[/:id]` + `POST
// /api/alert-methods/:id/test` (Task 6.6 backend,
// `fluxfang-api::alert_methods`). The list call was already here (Task 9.6's
// Entities page consumes it to populate the method multi-select on its "add
// alert rule" form — the response's `config` is already the safe,
// secret-free projection, `AlertMethodDto::config`, see that module's doc
// comment). Full management (create/edit/delete/test) is this task's
// (9.9) Alerts page.
//
// Per-type `config` shapes mirror the backend's decrypted-config structs
// (`notify::email::EmailConfig`/`notify::webhook::WebhookConfig`) that
// `validate_config_for_type` deserializes a submitted `config` into — every
// field on `EmailAlertMethodConfig` is required because `EmailConfig` has no
// `#[serde(default)]`s, whereas `WebhookAlertMethodConfig`'s `method`/
// `headers`/`secret` are optional to *submit* (the backend defaults them),
// though this page's form always sends them explicitly.
import { del, get, patch, post } from './client';

/** Mirrors `fluxfang-api::alert_methods::AlertMethodDto` — `config` is the
 * allowlisted, non-secret subset only (see that module's doc comment), not
 * the full stored config. */
export interface AlertMethod {
  id: string;
  name: string;
  type: string;
  enabled: boolean;
  created_at: string;
  config: Record<string, unknown>;
}

export type AlertMethodType = 'email' | 'webhook' | 'in_app';

/** `config` shape this page's form submits for `type: 'email'` — mirrors
 * `notify::email::EmailConfig` field-for-field (all required). */
export interface EmailAlertMethodConfig {
  host: string;
  port: number;
  username: string;
  password: string;
  from: string;
  to: string;
  tls: boolean;
}

/** `config` shape this page's form submits for `type: 'webhook'` — mirrors
 * `notify::webhook::WebhookConfig` (`method`/`headers`/`secret` are
 * optional on the wire, but this form always sends a value for each). */
export interface WebhookAlertMethodConfig {
  url: string;
  method: string;
  headers: Record<string, string>;
  secret: string;
}

/** `in_app` has no config at all (see `alert_methods.rs`'s module docs). */
export type InAppAlertMethodConfig = Record<string, never>;

export type AlertMethodConfig = EmailAlertMethodConfig | WebhookAlertMethodConfig | InAppAlertMethodConfig;

/** `POST /api/alert-methods` body — mirrors the backend's
 * `CreateAlertMethodRequest`. */
export interface CreateAlertMethodInput {
  name: string;
  type: AlertMethodType;
  enabled: boolean;
  config: AlertMethodConfig;
}

/** `PATCH /api/alert-methods/:id` body — mirrors `UpdateAlertMethodRequest`.
 * `type` is immutable after creation (see that route's doc comment), so
 * this never includes one; a resubmitted `config` is validated against the
 * existing row's type server-side. */
export interface UpdateAlertMethodInput {
  name?: string;
  enabled?: boolean;
  config?: Record<string, unknown>;
}

/** Mirrors `fluxfang-api::notify::DeliveryStatus`'s serde-tagged shape:
 * `{"status":"delivered"}` or `{"status":"failed","reason":"..."}`. */
export type DeliveryStatus = { status: 'delivered' } | { status: 'failed'; reason: string };

export function listAlertMethods(): Promise<AlertMethod[]> {
  return get<AlertMethod[]>('/api/alert-methods');
}

export function createAlertMethod(input: CreateAlertMethodInput): Promise<AlertMethod> {
  return post<AlertMethod>('/api/alert-methods', input);
}

export function updateAlertMethod(id: string, input: UpdateAlertMethodInput): Promise<AlertMethod> {
  return patch<AlertMethod>(`/api/alert-methods/${id}`, input);
}

export function deleteAlertMethod(id: string): Promise<void> {
  return del<void>(`/api/alert-methods/${id}`);
}

export function testAlertMethod(id: string): Promise<DeliveryStatus> {
  return post<DeliveryStatus>(`/api/alert-methods/${id}/test`);
}

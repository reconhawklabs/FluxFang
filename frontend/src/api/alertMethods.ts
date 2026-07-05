// `GET /api/alert-methods` (Task 6.6 backend, `fluxfang-api::alert_methods`).
// Only the list call: Task 9.6's Entities page consumes this to populate the
// method multi-select on its "add alert rule" form — the response's `config`
// is already the safe, secret-free projection (`AlertMethodDto::config`, see
// that module's doc comment), so this type never needs to model the
// per-type secret fields. Full Alert Methods management (create/edit/
// delete/test) belongs to the dedicated Alerts page (Task 9.9) — YAGNI here.
import { get } from './client';

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

export function listAlertMethods(): Promise<AlertMethod[]> {
  return get<AlertMethod[]>('/api/alert-methods');
}

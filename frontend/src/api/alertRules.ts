// `GET/POST/DELETE /api/alert-rules[/:id]` (Task 6.6 backend,
// `fluxfang-api::alert_rules`). Task 9.6's Entities page uses `list`/
// `create` to list an entity's existing rules (client-side filtered to
// `target_id === entity.id`) and to create a new entity-scoped rule from
// its "add alert" form. `delete` is added for this task's (9.9) Alerts
// page's read-only rules list, which offers a delete button as a
// nice-to-have. `PATCH /api/alert-rules/:id` still isn't needed by any
// slice (no rule *editing* UI exists yet) — YAGNI.
import type { Rule } from '../types/rule';
import { del, get, post } from './client';

/** The subset of the backend's `trigger.on` values this page's UI offers —
 * see `fluxfang-api::alert_rules`'s module doc comment for the full
 * validation matrix. `host_enters_zone`/`host_leaves_zone` require a null
 * `target_type`/`target_id` (a "host" rule, not scoped to any
 * emitter/entity), which this entity-scoped form never sends — but an
 * existing rule fetched back from `GET /api/alert-rules` could in principle
 * carry any of the backend's five values, so this type only constrains what
 * *this page* builds, not everything the wire can return. */
export type AlertRuleTriggerOn = 'detected' | 'enters_zone' | 'leaves_zone';

/** `trigger`'s shape — mirrors what `fluxfang-api::alert_rules::validate_trigger`
 * accepts. `zone_id` is required (and validated) only when `on` is a zone
 * trigger; `content_match`, if present, must be a well-formed `Rule` whose
 * conditions type-check against the `"wifi"` catalog. */
export interface AlertRuleTrigger {
  on: AlertRuleTriggerOn;
  zone_id?: string;
  content_match?: Rule;
}

/** Mirrors `fluxfang-api::alert_rules::AlertRuleDto`. */
export interface AlertRule {
  id: string;
  name: string;
  enabled: boolean;
  target_type: string | null;
  target_id: string | null;
  trigger: AlertRuleTrigger;
  method_ids: string[];
  created_at: string;
}

/** `POST /api/alert-rules` body — mirrors the backend's
 * `CreateAlertRuleRequest`. This page always sends `target_type: "entity"`
 * plus the entity's id. */
export interface CreateAlertRuleInput {
  name: string;
  enabled: boolean;
  target_type: 'entity' | 'emitter';
  target_id: string;
  trigger: AlertRuleTrigger;
  method_ids: string[];
}

export function listAlertRules(): Promise<AlertRule[]> {
  return get<AlertRule[]>('/api/alert-rules');
}

export function createAlertRule(input: CreateAlertRuleInput): Promise<AlertRule> {
  return post<AlertRule>('/api/alert-rules', input);
}

export function deleteAlertRule(id: string): Promise<void> {
  return del<void>(`/api/alert-rules/${id}`);
}

// The `Rule` wire shape produced by `RuleBuilder` (Task 9.2) and consumed by
// the backend's `fluxfang_core::rule::Rule`/`Condition` (see
// `fluxfang-api::emitters`'s `POST /api/emitters`, `POST
// /api/emitters/:id/rule`, and `GET /api/emitters/preview?rule=`).
//
// IMPORTANT: `Condition.value` must be typed per the referenced field's
// catalog `FieldType` (see `types/catalog.ts`) — a `number` field's value is
// a JSON number, not a numeric string; an `in` condition's value is an array
// of the field's typed elements. The backend's `conditions_to_sql_checked`
// rejects a mistyped value with a 400, so the UI (`ConditionRow` /
// `components/conditionUtils.ts`) is responsible for emitting the right
// JSON type — never a raw string coercion.

/** Matches the backend's `Op` enum (`#[serde(rename_all = "lowercase")]`,
 * with `In` renamed to `"in"`). Only ever set via a catalog field's `ops`
 * list — the UI never lets a user type one of these. */
export type Op = 'eq' | 'neq' | 'matches' | 'in' | 'gte' | 'lte';

/** Matches the backend's `MatchMode` enum. */
export type MatchMode = 'all' | 'any';

/** One condition row's value. `field` is empty only in a condition that
 * hasn't had a field chosen yet (shouldn't normally occur — `ConditionRow`
 * always seeds a real field key when a row is added); `op` is a valid `Op`
 * code once a field is chosen. `value`'s JSON type depends on the
 * referenced field's `FieldType` (string for text/mac/enum, number for
 * number, an array of the field's element type for `op === 'in'`). */
export interface Condition {
  field: string;
  op: Op | '';
  value: unknown;
}

/** The full wire shape: `{"match": "all"|"any", "conditions": [...]}`. */
export interface Rule {
  match: MatchMode;
  conditions: Condition[];
}

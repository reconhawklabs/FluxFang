// `FilterState` type + pure helpers for `FilterBar` (Task 9.2), split out of
// `FilterBar.tsx` itself so that component file only exports the component
// (oxlint's `react/only-export-components` — needed for React Fast Refresh
// to keep working on that file).
import type { Condition } from '../types/rule';
import { isCompleteCondition } from './conditionUtils';

/** Filter state `FilterBar` produces. Consumers (Task 9.4's Emissions page)
 * translate this into `GET /api/emissions` query params via
 * `filterToQueryParams` below rather than reaching into these fields
 * directly. */
export interface FilterState {
  q: string;
  conditions: Condition[];
  /** Mirrors the backend's `unassigned` bool param. `undefined`/`false`
   * means "no filtering on assignment"; `true` means "unassigned only". */
  unassigned?: boolean;
}

export const EMPTY_FILTER_STATE: FilterState = { q: '', conditions: [] };

/** Serialize one condition's typed value the way `parse_condition` in the
 * backend's `emissions.rs` expects: it parses each value token as JSON
 * first, falling back to treating it as a literal string only if that JSON
 * parse fails ("Parse-JSON-first, string-fallback" — see that module's doc
 * comment). That means a bare numeric-looking *string* value (e.g. a `Text`
 * field holding `"2024"`, or `"true"`/`"false"`/`"null"`) would be
 * mis-parsed as a JSON number/bool/null instead of the string it actually
 * is, tripping the backend's `conditions_to_sql_checked` type check with a
 * 400 `InvalidValueType`.
 *
 * So we key the encoding off the value's *JS runtime type* rather than
 * blindly stringifying: a genuine JS `number` is emitted bare (`String(6)`
 * -> `6`, a valid bare JSON number token the backend parses back as a
 * number), while anything else scalar (string/boolean) goes through
 * `JSON.stringify` (`"2024"`, `true`) so the backend's JSON parse yields
 * that same string/bool rather than silently coercing it to a number. This
 * is sound because `RuleBuilder`/`ConditionRow` already hand us
 * correctly-typed JS values per the field's catalog type (a `number` for a
 * `number` field, a `string` otherwise) — so `typeof` alone is enough,
 * without threading the field catalog through here to look up the
 * declared type. `in` conditions serialize as a JSON array with each
 * element typed the same way (numbers bare, strings quoted) — precisely
 * what `JSON.stringify` on a JS array already produces. */
function conditionValueToken(condition: Condition): string {
  if (Array.isArray(condition.value)) return JSON.stringify(condition.value);
  if (typeof condition.value === 'number') return String(condition.value);
  return JSON.stringify(condition.value);
}

/**
 * The `cond=field:op:value` tokens (each already `field:op:value`-joined,
 * ready to `params.append('cond', token)`) for every *complete* condition in
 * `conditions` — incomplete rows (no field/op chosen yet, or an empty
 * value) are silently omitted, same rule `isCompleteCondition` uses for the
 * preview gate in `RuleBuilder`. Shared by `filterToQueryParams` below and
 * by `StackedFilterBuilder`'s `conditionsToQueryParams` (Phase 2) so both
 * encode a condition list identically.
 */
export function conditionsToCondParams(conditions: Condition[]): string[] {
  return conditions
    .filter(isCompleteCondition)
    .map((condition) => `${condition.field}:${condition.op}:${conditionValueToken(condition)}`);
}

/**
 * Translate a `FilterState` into `URLSearchParams` for `GET /api/emissions`
 * (Task 9.4): `q=<text>` when non-empty, `unassigned=true` when set, and one
 * repeated `cond=field:op:value` per *complete* condition (via
 * `conditionsToCondParams`).
 */
export function filterToQueryParams(state: FilterState): URLSearchParams {
  const params = new URLSearchParams();

  const q = state.q.trim();
  if (q.length > 0) params.set('q', q);

  if (state.unassigned) params.set('unassigned', 'true');

  for (const token of conditionsToCondParams(state.conditions)) {
    params.append('cond', token);
  }

  return params;
}

/**
 * `StackedFilterBuilder` (Phase 2) works with a bare `Condition[]` (no
 * `q`/`unassigned` — those live on the Emissions page's own `SearchBar`/
 * data-source dropdown instead), so it gets its own params helper rather
 * than reusing `filterToQueryParams`'s `FilterState`-shaped input: one
 * repeated `cond=field:op:value` per complete condition, same encoding
 * (`conditionsToCondParams`) as the rest of this module.
 */
export function conditionsToQueryParams(conditions: Condition[]): URLSearchParams {
  const params = new URLSearchParams();
  for (const token of conditionsToCondParams(conditions)) {
    params.append('cond', token);
  }
  return params;
}

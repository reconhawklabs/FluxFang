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
 * backend's `emissions.rs` expects: a JSON array for `in` (e.g.
 * `[1,2]`/`["beacon","probe_request"]`), and the bare value otherwise —
 * `String(number)` is a valid bare JSON number token, and any other string
 * that isn't itself valid JSON falls back to being used as a literal string
 * (see that module's "Parse-JSON-first, string-fallback" doc comment). */
function conditionValueToken(condition: Condition): string {
  return Array.isArray(condition.value) ? JSON.stringify(condition.value) : String(condition.value);
}

/**
 * Translate a `FilterState` into `URLSearchParams` for `GET /api/emissions`
 * (Task 9.4): `q=<text>` when non-empty, `unassigned=true` when set, and one
 * repeated `cond=field:op:value` per *complete* condition (incomplete rows —
 * no field/op chosen yet, or an empty value — are silently omitted, same
 * rule `isCompleteCondition` uses for the preview gate in `RuleBuilder`).
 */
export function filterToQueryParams(state: FilterState): URLSearchParams {
  const params = new URLSearchParams();

  const q = state.q.trim();
  if (q.length > 0) params.set('q', q);

  if (state.unassigned) params.set('unassigned', 'true');

  for (const condition of state.conditions) {
    if (!isCompleteCondition(condition)) continue;
    params.append('cond', `${condition.field}:${condition.op}:${conditionValueToken(condition)}`);
  }

  return params;
}

// Pure helpers shared by `ConditionRow`, `RuleBuilder`, and `FilterBar`
// (Task 9.2) — the single place that decides how a condition's `value` gets
// re-typed as the user changes `field`/`op`. Kept side-effect-free and unit
// testable on their own, independent of any rendering.
import type { FieldDef, FieldType } from '../types/catalog';
import type { Condition, Op } from '../types/rule';

/** The first op a freshly-chosen field offers, i.e. what a new condition (or
 * a condition whose field just changed) defaults to. Empty-ops fields (only
 * possible for an unknown/empty catalog) fall back to `''`. */
export function firstOpFor(field: FieldDef): Op | '' {
  return (field.ops[0]?.code as Op | undefined) ?? '';
}

/** The value a condition should reset to when `field`/`op` land on a fresh
 * combination: `[]` for `in` (a multi-value input), the first enum value for
 * an enum field, `''` otherwise (empty text/number input, typed on entry). */
export function defaultValueFor(field: FieldDef, op: Op | ''): unknown {
  if (op === 'in') return [];
  if (field.type === 'enum') return field.values?.[0] ?? '';
  return '';
}

/** Build a brand-new condition for `field`, defaulting to its first op and
 * that op's default value — what "Add condition"/a field-dropdown change
 * produces. */
export function newConditionFor(field: FieldDef): Condition {
  const op = firstOpFor(field);
  return { field: field.key, op, value: defaultValueFor(field, op) };
}

/** Re-shape `oldValue` when the operator changes to/from `in`: switching
 * *to* `in` wraps a scalar into a single-element array (or `[]` if there was
 * nothing to wrap); switching *away from* `in` unwraps the array's first
 * element (or the field's default if the array was empty). Any other op
 * change (e.g. `eq` -> `neq`) leaves the value untouched — both are scalar
 * ops. */
export function adaptValueForOp(field: FieldDef | undefined, newOp: Op | '', oldValue: unknown): unknown {
  const goingToIn = newOp === 'in';
  const wasArray = Array.isArray(oldValue);

  if (goingToIn) {
    if (wasArray) return oldValue;
    if (oldValue === '' || oldValue === undefined || oldValue === null) return [];
    return [oldValue];
  }

  if (wasArray) {
    const arr = oldValue as unknown[];
    if (arr.length > 0) return arr[0];
    return field ? defaultValueFor(field, newOp) : '';
  }

  return oldValue;
}

/** Parse a comma-separated string into a typed array for an `in` condition:
 * numbers are `Number()`-parsed (non-numeric tokens are dropped rather than
 * emitting `NaN`), everything else (text/mac/enum) stays a string array. */
export function parseMultiValue(raw: string, type: FieldType): unknown[] {
  const parts = raw
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  if (type === 'number') {
    return parts.map(Number).filter((n) => !Number.isNaN(n));
  }
  return parts;
}

/** Render a condition's current value back into the comma-separated text an
 * `in` input (or any plain display) shows. */
export function formatValueForDisplay(value: unknown): string {
  if (Array.isArray(value)) return value.join(', ');
  if (value === undefined || value === null) return '';
  return String(value);
}

/** A condition is "complete" (safe to send to the backend / include in a
 * preview or filter query) once it has a field, an op, and a non-empty
 * value — an empty string (untouched text/number input) or empty array (an
 * `in` condition with no entries yet) doesn't count. */
export function isCompleteCondition(condition: Condition): boolean {
  if (!condition.field || !condition.op) return false;
  if (Array.isArray(condition.value)) return condition.value.length > 0;
  return condition.value !== '' && condition.value !== undefined && condition.value !== null;
}

// Wire types for `GET /api/catalog/:kind` (Task 6.1 backend, `FieldDefDto` in
// `fluxfang-api::dto`). Drives every field/operator dropdown in
// `RuleBuilder`/`FilterBar` (Task 9.2) — nothing here is invented client
// state, it's a direct mirror of what the backend serializes.

/** The value domain of a catalog field — decides which value input widget
 * `ConditionRow` renders (text/number/enum-select/mac) and, together with
 * `ops`, which operators make sense. */
export type FieldType = 'text' | 'mac' | 'number' | 'enum';

/** One operator as exposed by the catalog: `code` is the wire value that
 * goes into a `Condition.op` (and must never be shown to the user), `label`
 * is the plain-English text the operator dropdown displays. */
export interface FieldOp {
  code: string;
  label: string;
}

/** One field a `kind`'s catalog exposes. `values` is present only when
 * `type === 'enum'` (mirrors the backend DTO's `skip_serializing_if`). */
export interface FieldDef {
  key: string;
  label: string;
  type: FieldType;
  values?: string[];
  ops: FieldOp[];
}

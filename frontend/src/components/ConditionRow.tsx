// One field-dropdown -> operator-dropdown -> type-aware-value-input row,
// shared by `RuleBuilder` and `FilterBar` (Task 9.2) so both stay
// consistent. The user only ever picks from dropdowns/typed inputs here —
// no query syntax is ever typed.
import type { ChangeEvent } from 'react';
import type { FieldDef } from '../types/catalog';
import type { Condition, Op } from '../types/rule';
import { adaptValueForOp, formatValueForDisplay, newConditionFor, parseMultiValue } from './conditionUtils';

export interface ConditionRowProps {
  /** The full catalog for the current `kind` — the field dropdown's
   * options, and (via the matched field) the operator dropdown's options
   * and the value input's shape. */
  fields: FieldDef[];
  condition: Condition;
  onChange: (next: Condition) => void;
  /** Omit to render a row with no remove button (not used today, but keeps
   * the component usable for a fixed single-condition editor later). */
  onRemove?: () => void;
  /** This row's position, purely for generating unique element ids/labels
   * so multiple rows on one page don't collide (e.g. "Field (condition
   * 2)"). Display purposes only — has no effect on the emitted `Condition`. */
  index: number;
  /** Distinguishes this row's element ids/`data-testid` from another
   * condition-row list rendered on the same page at the same `index` —
   * e.g. the Emissions page's `StackedFilterBuilder` (no prefix) rendering
   * alongside its "Assign to emitter" modal's `RuleBuilder` (prefixed
   * `"rule-"`), both of which can have a row at index 0 open at once.
   * Defaults to `''` (existing single-instance-per-page callers are
   * unaffected). */
  idPrefix?: string;
}

const selectClassName =
  'rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const inputClassName = selectClassName;

export default function ConditionRow({ fields, condition, onChange, onRemove, index, idPrefix = '' }: ConditionRowProps) {
  const field = fields.find((f) => f.key === condition.field);
  const rowLabel = `condition ${index + 1}`;
  const idBase = `${idPrefix}condition-row-${index}`;

  function handleFieldChange(event: ChangeEvent<HTMLSelectElement>): void {
    const nextField = fields.find((f) => f.key === event.target.value);
    if (!nextField) return;
    onChange(newConditionFor(nextField));
  }

  function handleOpChange(event: ChangeEvent<HTMLSelectElement>): void {
    const nextOp = event.target.value as Op;
    onChange({ ...condition, op: nextOp, value: adaptValueForOp(field, nextOp, condition.value) });
  }

  function handleValueChange(raw: string): void {
    if (condition.op === 'in') {
      onChange({ ...condition, value: parseMultiValue(raw, field?.type ?? 'text') });
      return;
    }
    if (field?.type === 'number') {
      onChange({ ...condition, value: raw === '' ? '' : Number(raw) });
      return;
    }
    onChange({ ...condition, value: raw });
  }

  function renderValueInput() {
    const id = `${idBase}-value`;
    const label = `Value (${rowLabel})`;

    if (condition.op === 'in') {
      return (
        <>
          <label htmlFor={id} className="sr-only">
            {label}
          </label>
          <input
            id={id}
            type="text"
            value={formatValueForDisplay(condition.value)}
            onChange={(event) => handleValueChange(event.target.value)}
            placeholder="comma-separated values"
            className={inputClassName}
          />
        </>
      );
    }

    if (field?.type === 'enum') {
      return (
        <>
          <label htmlFor={id} className="sr-only">
            {label}
          </label>
          <select
            id={id}
            value={typeof condition.value === 'string' ? condition.value : ''}
            onChange={(event) => handleValueChange(event.target.value)}
            className={selectClassName}
          >
            {(field.values ?? []).map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        </>
      );
    }

    if (field?.type === 'number') {
      return (
        <>
          <label htmlFor={id} className="sr-only">
            {label}
          </label>
          <input
            id={id}
            type="number"
            value={condition.value === '' || condition.value === undefined ? '' : String(condition.value)}
            onChange={(event) => handleValueChange(event.target.value)}
            className={inputClassName}
          />
        </>
      );
    }

    return (
      <>
        <label htmlFor={id} className="sr-only">
          {label}
        </label>
        <input
          id={id}
          type="text"
          value={typeof condition.value === 'string' ? condition.value : ''}
          onChange={(event) => handleValueChange(event.target.value)}
          placeholder={field?.type === 'mac' ? 'AA:BB:CC:DD:EE:FF' : undefined}
          className={inputClassName}
        />
      </>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-2" data-testid={idBase}>
      <label htmlFor={`${idBase}-field`} className="sr-only">
        {`Field (${rowLabel})`}
      </label>
      <select
        id={`${idBase}-field`}
        value={condition.field}
        onChange={handleFieldChange}
        className={selectClassName}
      >
        {fields.map((f) => (
          <option key={f.key} value={f.key}>
            {f.label}
          </option>
        ))}
      </select>

      <label htmlFor={`${idBase}-op`} className="sr-only">
        {`Operator (${rowLabel})`}
      </label>
      <select id={`${idBase}-op`} value={condition.op} onChange={handleOpChange} className={selectClassName}>
        {(field?.ops ?? []).map((o) => (
          <option key={o.code} value={o.code}>
            {o.label}
          </option>
        ))}
      </select>

      {renderValueInput()}

      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          aria-label={`Remove ${rowLabel}`}
          className="text-sm text-slate-500 transition hover:text-red-400"
        >
          Remove
        </button>
      )}
    </div>
  );
}

// Compact, catalog-driven filter bar (Task 9.2). Reuses the same
// `useCatalog`/`ConditionRow` machinery as `RuleBuilder` so field/operator
// dropdowns stay consistent between "build a rule" and "filter a list", plus
// a free-text search box (`q`) and a simple "unassigned only" toggle
// (mirrors the backend's `GET /api/emissions?unassigned=true` param — see
// `fluxfang-api::emissions::parse_filter`).
//
// Deliberately does NOT include data-source/session/time/kind selectors —
// per the Task 9.2 brief those are for the Emissions page (Task 9.4) to add
// on top of this if/when it needs them; this component's job stops at
// field-condition + text (+ unassigned) filters.
import { useCatalog } from '../hooks/useCatalog';
import type { Condition } from '../types/rule';
import ConditionRow from './ConditionRow';
import { newConditionFor } from './conditionUtils';
import type { FilterState } from './filterState';

export interface FilterBarProps {
  kind: string;
  value: FilterState;
  onChange: (next: FilterState) => void;
}

export default function FilterBar({ kind, value, onChange }: FilterBarProps) {
  const { data: fields } = useCatalog(kind);

  function handleAddCondition(): void {
    if (!fields || fields.length === 0) return;
    onChange({ ...value, conditions: [...value.conditions, newConditionFor(fields[0])] });
  }

  function handleConditionChange(index: number, next: Condition): void {
    onChange({ ...value, conditions: value.conditions.map((c, i) => (i === index ? next : c)) });
  }

  function handleRemoveCondition(index: number): void {
    onChange({ ...value, conditions: value.conditions.filter((_, i) => i !== index) });
  }

  return (
    <div className="flex flex-wrap items-end gap-3 rounded border border-slate-800 bg-slate-900/40 p-3">
      <div className="flex flex-col gap-0.5">
        <label htmlFor="filter-q" className="text-xs font-medium uppercase tracking-wide text-slate-500">
          Search
        </label>
        <input
          id="filter-q"
          type="text"
          value={value.q}
          onChange={(event) => onChange({ ...value, q: event.target.value })}
          placeholder="Search…"
          className="rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
        />
      </div>

      <label className="flex items-center gap-1.5 text-sm text-slate-400">
        <input
          type="checkbox"
          checked={value.unassigned ?? false}
          onChange={(event) => onChange({ ...value, unassigned: event.target.checked })}
          className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
        />
        Unassigned only
      </label>

      {fields &&
        value.conditions.map((condition, index) => (
          <ConditionRow
            key={index}
            fields={fields}
            condition={condition}
            index={index}
            onChange={(next) => handleConditionChange(index, next)}
            onRemove={() => handleRemoveCondition(index)}
          />
        ))}

      {fields && (
        <button
          type="button"
          onClick={handleAddCondition}
          disabled={fields.length === 0}
          className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          + Add filter
        </button>
      )}
    </div>
  );
}

// The progressive "stacked" filter UI (Phase 2 of the list-pages UX cleanup)
// — shared by Emissions now, Emitters/Entities in Phases 3/4 wherever they
// need field-condition filters. Reuses the exact same `useCatalog`/
// `ConditionRow`/`conditionUtils` machinery as `RuleBuilder`/`FilterBar`
// (Task 9.2) so field/operator/value dropdowns stay consistent everywhere
// a rule or filter is built — no new operator-typing or value-parsing logic
// lives here.
//
// Behavior (see the design doc's "Frontend shared patterns" section): the
// first condition row always renders, even before the caller's `value` has
// anything in it; once a row is *complete* (`isCompleteCondition` — field +
// op + a non-empty value), an "Add Additional Filter" button appears below
// it; clicking it appends a fresh empty row (which hides the button again
// until that new row is itself completed).
//
// The always-visible first row is rendered without ever mutating `value`
// via an effect: `effectiveConditions` derives a fresh placeholder condition
// (`newConditionFor(fields[0])`) whenever `value` is empty, the same
// "derive a sensible default instead of syncing it into state" trick
// `Emissions.tsx`'s `AssignModal` already uses for its Type `<select>`
// (`effectiveTypeSelection`). Editing that placeholder is what turns it
// into `value[0]` — `handleConditionChange` always maps over
// `effectiveConditions`, so the very first edit is the moment the caller's
// `value` goes from `[]` to a real one-condition array.
import { useCatalog } from '../hooks/useCatalog';
import type { Condition } from '../types/rule';
import ConditionRow from './ConditionRow';
import { isCompleteCondition, newConditionFor } from './conditionUtils';

export interface StackedFilterBuilderProps {
  /** Data source kind whose catalog (`GET /api/catalog/:kind`) drives the
   * field/operator/value dropdowns, e.g. `"wifi"`. */
  kind: string;
  value: Condition[];
  onChange: (next: Condition[]) => void;
}

export default function StackedFilterBuilder({ kind, value, onChange }: StackedFilterBuilderProps) {
  const { data: fields, isLoading, isError } = useCatalog(kind);

  const effectiveConditions: Condition[] =
    value.length > 0 ? value : fields && fields.length > 0 ? [newConditionFor(fields[0])] : [];

  function handleConditionChange(index: number, next: Condition): void {
    onChange(effectiveConditions.map((c, i) => (i === index ? next : c)));
  }

  function handleRemoveCondition(index: number): void {
    // Nothing persisted yet to remove — the sole row is still the derived
    // placeholder, not something in `value`.
    if (value.length === 0) return;
    onChange(value.filter((_, i) => i !== index));
  }

  function handleAddCondition(): void {
    if (!fields || fields.length === 0) return;
    onChange([...effectiveConditions, newConditionFor(fields[0])]);
  }

  const lastCondition = effectiveConditions[effectiveConditions.length - 1];
  const showAddButton = effectiveConditions.length > 0 && isCompleteCondition(lastCondition);

  return (
    <div className="space-y-2">
      {isLoading && <p className="text-sm text-slate-500">Loading fields…</p>}
      {isError && <p className="text-sm text-red-400">Failed to load the &quot;{kind}&quot; field catalog.</p>}

      {fields &&
        effectiveConditions.map((condition, index) => (
          <ConditionRow
            key={index}
            fields={fields}
            condition={condition}
            index={index}
            onChange={(next) => handleConditionChange(index, next)}
            onRemove={value.length > 0 || index > 0 ? () => handleRemoveCondition(index) : undefined}
          />
        ))}

      {fields && showAddButton && (
        <button
          type="button"
          onClick={handleAddCondition}
          className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400"
        >
          + Add Additional Filter
        </button>
      )}
    </div>
  );
}

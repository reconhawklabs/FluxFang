// Catalog-driven, all-dropdown rule editor (Task 9.2). Renders a match-mode
// toggle + a list of `ConditionRow`s (field dropdown -> operator dropdown ->
// type-aware value input) sourced from `useCatalog(kind)`, and — when
// `showPreview` is set — a debounced live match count from
// `GET /api/emitters/preview?rule=`.
//
// Fully controlled: `value`/`onChange` are the only source of truth for the
// rule being edited. Nothing here holds its own copy of `conditions` —
// every row's edit calls `onChange` with a brand-new `Rule`, same as any
// other controlled form input.
import { useQuery } from '@tanstack/react-query';
import { get } from '../api/client';
import { useCatalog } from '../hooks/useCatalog';
import { useDebouncedValue } from '../hooks/useDebouncedValue';
import type { Condition, MatchMode, Rule } from '../types/rule';
import ConditionRow from './ConditionRow';
import { isCompleteCondition, newConditionFor } from './conditionUtils';

export interface RuleBuilderProps {
  /** Data source kind whose catalog (`GET /api/catalog/:kind`) drives the
   * field/operator/value dropdowns, e.g. `"wifi"`. */
  kind: string;
  value: Rule;
  onChange: (next: Rule) => void;
  /** When true, renders a live "Matches N emissions" count from
   * `GET /api/emitters/preview?rule=`, debounced and only queried once the
   * rule has at least one complete condition. Defaults to false — most
   * embeddings (e.g. an inline editor inside a bigger form) don't want a
   * network request firing on every keystroke. */
  showPreview?: boolean;
}

interface MatchCountResponse {
  match_count: number;
}

/** How long to wait after the last edit before firing a preview request. */
const PREVIEW_DEBOUNCE_MS = 400;

export default function RuleBuilder({ kind, value, onChange, showPreview = false }: RuleBuilderProps) {
  const { data: fields, isLoading, isError } = useCatalog(kind);

  function handleMatchModeChange(mode: MatchMode): void {
    onChange({ ...value, match: mode });
  }

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

  const hasCompleteCondition = value.conditions.some(isCompleteCondition);
  const debouncedRule = useDebouncedValue(value, PREVIEW_DEBOUNCE_MS);
  const previewEnabled = showPreview && hasCompleteCondition;

  const previewQuery = useQuery({
    queryKey: ['emitters', 'preview', kind, debouncedRule],
    queryFn: () =>
      get<MatchCountResponse>(`/api/emitters/preview?rule=${encodeURIComponent(JSON.stringify(debouncedRule))}`),
    enabled: previewEnabled,
  });

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <label htmlFor="rule-match-mode" className="text-xs font-medium uppercase tracking-wide text-slate-500">
          Match
        </label>
        <select
          id="rule-match-mode"
          value={value.match}
          onChange={(event) => handleMatchModeChange(event.target.value as MatchMode)}
          className="rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
        >
          <option value="all">Match ALL</option>
          <option value="any">Match ANY</option>
        </select>
      </div>

      {isLoading && <p className="text-sm text-slate-500">Loading fields…</p>}
      {isError && <p className="text-sm text-red-400">Failed to load the &quot;{kind}&quot; field catalog.</p>}

      {fields && (
        <div className="space-y-2">
          {value.conditions.map((condition, index) => (
            <ConditionRow
              key={index}
              fields={fields}
              condition={condition}
              index={index}
              onChange={(next) => handleConditionChange(index, next)}
              onRemove={() => handleRemoveCondition(index)}
            />
          ))}

          <button
            type="button"
            onClick={handleAddCondition}
            disabled={fields.length === 0}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            + Add condition
          </button>
        </div>
      )}

      {showPreview && (
        <p className="text-sm text-slate-400">
          {!hasCompleteCondition
            ? 'Add a condition to preview matches.'
            : previewQuery.isLoading
              ? 'Checking matches…'
              : previewQuery.isError
                ? 'Could not check matches.'
                : `Matches ${previewQuery.data?.match_count ?? 0} emissions`}
        </p>
      )}
    </div>
  );
}

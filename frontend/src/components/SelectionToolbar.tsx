// Mass-select action bar (Phase 2 of the list-pages UX cleanup) — pairs with
// `useRowSelection`. Two destructive actions, both gated behind a
// `window.confirm` (per the design doc's "Destructive actions ... require
// an explicit confirm" constraint): "Delete selected (N)" (disabled at
// N===0) and "Clear All <itemLabelPlural>" (always enabled — it doesn't
// depend on any selection).
//
// Deliberately dumb: this component doesn't know how deletion actually
// happens (bulk-delete endpoint, id list, refetch, ...) — it just confirms
// and calls the callback the caller supplied, same as the assign/clear
// mutations already living on each list page.
export interface SelectionToolbarProps {
  selectedCount: number;
  onDeleteSelected: () => void;
  onClearAll: () => void;
  /** Plural noun for this page's rows, e.g. "emissions" — used to build
   * both confirm-dialog messages and the "Clear All ..." button's label
   * (title-cased as given, so pass "Emissions"/"Emitters"/"Entities"). */
  itemLabelPlural: string;
}

export default function SelectionToolbar({
  selectedCount,
  onDeleteSelected,
  onClearAll,
  itemLabelPlural,
}: SelectionToolbarProps) {
  const lowerLabel = itemLabelPlural.toLowerCase();

  function handleDeleteSelected(): void {
    const confirmed = window.confirm(
      `Delete ${selectedCount} selected ${lowerLabel}? This cannot be undone.`,
    );
    if (confirmed) onDeleteSelected();
  }

  function handleClearAll(): void {
    const confirmed = window.confirm(`Delete ALL ${lowerLabel}? This cannot be undone.`);
    if (confirmed) onClearAll();
  }

  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        disabled={selectedCount === 0}
        onClick={handleDeleteSelected}
        className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Delete selected ({selectedCount})
      </button>
      <button
        type="button"
        onClick={handleClearAll}
        className="rounded border border-red-900 px-3 py-1.5 text-sm text-red-400 transition hover:border-red-600 hover:text-red-300"
      >
        Clear All {itemLabelPlural}
      </button>
    </div>
  );
}

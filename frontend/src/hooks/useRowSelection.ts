// Mass-select state for a list page's table (Phase 2 of the list-pages UX
// cleanup — shared by Emissions now, Emitters/Entities in Phases 3/4). A
// thin `Set<string>` of selected row ids plus the handful of operations a
// header select-all checkbox + per-row checkboxes need.
//
// `ids` is the current page's row ids (passed in so `allSelected` can tell
// whether every visible row is checked); `toggleAll` takes its own `ids`
// argument too rather than silently closing over the hook's — in practice
// callers always pass the same array, but keeping it explicit means the
// header checkbox's "select every row I'm about to toggle" intent reads
// directly at the call site instead of needing a trip back to the hook call.
import { useCallback, useState } from 'react';

export interface RowSelection {
  /** The selected row ids. A `ReadonlySet` (not a plain array) so callers
   * get O(1) `.has()` checks for each row's checkbox without re-deriving a
   * lookup structure every render. */
  selected: ReadonlySet<string>;
  toggle: (id: string) => void;
  /** Selects every id in `ids` if any is currently unselected, otherwise
   * (all of `ids` already selected) clears just those ids — the usual
   * header-checkbox "select all / none" toggle semantics. */
  toggleAll: (ids: string[]) => void;
  clear: () => void;
  /** Whether every id in the hook's `ids` (the current page) is selected.
   * `false` when `ids` is empty (nothing to be "all selected" of). */
  allSelected: boolean;
}

export function useRowSelection(ids: string[]): RowSelection {
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());

  const toggle = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const toggleAll = useCallback((allIds: string[]) => {
    setSelected((prev) => {
      const allCurrentlySelected = allIds.length > 0 && allIds.every((id) => prev.has(id));
      return allCurrentlySelected ? new Set() : new Set(allIds);
    });
  }, []);

  const clear = useCallback(() => setSelected(new Set()), []);

  const allSelected = ids.length > 0 && ids.every((id) => selected.has(id));

  return { selected, toggle, toggleAll, clear, allSelected };
}

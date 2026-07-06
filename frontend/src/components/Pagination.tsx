// Page-size select + prev/next + "N-M of TOTAL" label (Phase 2 of the
// list-pages UX cleanup) — extracted from the original `Emissions.tsx`'s
// inline pagination footer so Emitters/Entities (Phases 3/4) render an
// identical control instead of re-deriving the same bounds math.
//
// Fully controlled, same convention as `RuleBuilder`/`FilterBar`: this
// component holds no state of its own, `onChange(nextLimit, nextOffset)` is
// the only way `limit`/`offset` move, and every button here computes the
// next `(limit, offset)` pair itself (page-size change resets to offset 0,
// same as the original Emissions page's `handlePageSizeChange` did).
export interface PaginationProps {
  total: number;
  limit: number;
  offset: number;
  onChange: (limit: number, offset: number) => void;
  /** Defaults to the Emissions page's original options. */
  pageSizeOptions?: readonly number[];
}

const DEFAULT_PAGE_SIZE_OPTIONS = [25, 50, 100, 200] as const;

export default function Pagination({
  total,
  limit,
  offset,
  onChange,
  pageSizeOptions = DEFAULT_PAGE_SIZE_OPTIONS,
}: PaginationProps) {
  const pageStart = total === 0 ? 0 : offset + 1;
  const pageEnd = Math.min(offset + limit, total);

  return (
    <div className="flex items-center justify-between text-sm text-slate-400">
      <div className="flex items-center gap-2">
        <label htmlFor="pagination-page-size" className="text-xs font-medium uppercase tracking-wide text-slate-500">
          Page size
        </label>
        <select
          id="pagination-page-size"
          value={limit}
          onChange={(event) => onChange(Number(event.target.value), 0)}
          className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
        >
          {pageSizeOptions.map((size) => (
            <option key={size} value={size}>
              {size}
            </option>
          ))}
        </select>
      </div>

      <div className="flex items-center gap-3">
        <span>
          {pageStart}–{pageEnd} of {total}
        </span>
        <button
          type="button"
          disabled={offset === 0}
          onClick={() => onChange(limit, Math.max(0, offset - limit))}
          className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Prev
        </button>
        <button
          type="button"
          disabled={offset + limit >= total}
          onClick={() => onChange(limit, offset + limit)}
          className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Next
        </button>
      </div>
    </div>
  );
}

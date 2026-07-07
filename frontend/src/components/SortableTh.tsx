export type SortDir = "asc" | "desc";

interface SortableThProps {
  label: string;
  sortKey: string;
  activeKey: string | null;
  activeDir: SortDir;
  onSort: (key: string) => void;
  className?: string;
}

/** A clickable table header that reports sort intent. First click on an
 * inactive column sorts it ascending; the parent toggles direction when the
 * active column is clicked again. Shows ↑/↓ + aria-sort for the active
 * column. */
export function SortableTh({
  label,
  sortKey,
  activeKey,
  activeDir,
  onSort,
  className,
}: SortableThProps) {
  const active = activeKey === sortKey;
  return (
    <th
      className={className ?? "py-2 pr-4 font-medium"}
      aria-sort={active ? (activeDir === "asc" ? "ascending" : "descending") : "none"}
    >
      <button
        type="button"
        onClick={() => onSort(sortKey)}
        className="flex items-center gap-1 uppercase tracking-wide text-slate-500 hover:text-slate-300"
      >
        {label}
        <span aria-hidden="true" className="text-amber-400">
          {active ? (activeDir === "asc" ? "↑" : "↓") : ""}
        </span>
      </button>
    </th>
  );
}

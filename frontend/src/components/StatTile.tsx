// Small reusable KPI stat card used by the Dashboard's at-a-glance count
// row (Task 9.10) — a prominent number over a label, dark-themed with the
// app's amber accent. Kept generic (no Dashboard-specific knowledge) in
// case a later page wants the same "count tile" look.
export interface StatTileProps {
  label: string;
  /** `null`/`undefined` renders a loading placeholder ("—") instead of a
   * number, so a tile never flashes `0` while its query is still in
   * flight. */
  value: number | null | undefined;
  loading?: boolean;
}

export default function StatTile({ label, value, loading }: StatTileProps) {
  const display = loading || value === null || value === undefined ? '—' : value.toLocaleString();

  return (
    <div
      data-testid={`stat-tile-${label}`}
      className="rounded-lg border border-slate-800 bg-slate-900/60 px-4 py-3"
    >
      <p className="text-2xl font-semibold text-amber-400">{display}</p>
      <p className="mt-1 text-xs font-medium uppercase tracking-wide text-slate-500">{label}</p>
    </div>
  );
}

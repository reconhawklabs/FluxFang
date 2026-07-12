// Slide-over panel listing every emitter hidden from the Co-Travel page, each
// with a Restore button. Opened from the page header's "Ignored (N)" link.
// Restoring un-ignores and refreshes both the co-travel list and this list.
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '../api/queryKeys';
import { listIgnored, unignoreEmitter } from '../api/coTravel';

export interface IgnoredDrawerProps {
  open: boolean;
  onClose: () => void;
}

export default function IgnoredDrawer({ open, onClose }: IgnoredDrawerProps) {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: queryKeys.coTravelIgnored,
    queryFn: listIgnored,
    enabled: open,
  });
  const restore = useMutation({
    mutationFn: (emitterId: string) => unignoreEmitter(emitterId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.coTravel });
      void qc.invalidateQueries({ queryKey: queryKeys.coTravelIgnored });
    },
  });

  if (!open) return null;
  const items = query.data ?? [];

  return (
    <div className="fixed inset-0 z-40" role="dialog" aria-label="Ignored emitters">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} aria-hidden="true" />
      <aside className="absolute right-0 top-0 flex h-full w-80 flex-col border-l border-slate-800 bg-slate-900 p-4">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-slate-100">Ignored ({items.length})</h2>
          <button
            type="button"
            onClick={onClose}
            className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-slate-500 hover:text-slate-100"
          >
            Close
          </button>
        </div>
        {items.length === 0 ? (
          <p className="text-xs text-slate-500">Nothing ignored.</p>
        ) : (
          <ul className="flex-1 divide-y divide-slate-800 overflow-auto">
            {items.map((e) => (
              <li key={e.id} className="flex items-center justify-between gap-2 py-2">
                <span className="truncate text-xs text-slate-300">
                  <span className="text-slate-500">{e.emitter_type ?? 'emitter'}</span>
                  <span> · </span>
                  <span>{e.identity_key ?? e.name}</span>
                </span>
                <button
                  type="button"
                  onClick={() => restore.mutate(e.id)}
                  disabled={restore.isPending}
                  className="shrink-0 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100 disabled:opacity-50"
                >
                  Restore
                </button>
              </li>
            ))}
          </ul>
        )}
      </aside>
    </div>
  );
}

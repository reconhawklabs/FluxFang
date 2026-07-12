// One ranked emitter row on the Co-Travel page. Owns its own expand state; the
// Details button lazily mounts CoTravelDetails (map + sparkline). Ignore is
// delegated to the page via onIgnore so the mutation lives in one place.
import { useState } from 'react';
import type { CoTravelItem } from '../api/coTravel';
import CoTravelDetails from './CoTravelDetails';

export interface CoTravelRowProps {
  item: CoTravelItem;
  from?: string;
  to?: string;
  onIgnore: (emitterId: string) => void;
  ignoring?: boolean;
}

function miles(m: number): string {
  return `${(m / 1609.34).toFixed(1)} mi`;
}
function minutes(s: number): string {
  return `${(s / 60).toFixed(1)} min`;
}

export default function CoTravelRow({ item, from, to, onIgnore, ignoring }: CoTravelRowProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <li className="px-4 py-3">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="truncate text-sm text-slate-100">
            {item.emitter_type ?? 'emitter'} · <span>{item.identity_key ?? item.name}</span>
          </div>
          <div className="text-xs text-slate-400">
            {miles(item.spread_m)} spread · {item.points} points · {minutes(item.span_s)} ·{' '}
            {item.hits} hits · score {item.score}
          </div>
        </div>
        <div className="flex shrink-0 gap-2">
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="rounded border border-slate-700 px-3 py-1 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            {expanded ? 'Hide' : 'Details'}
          </button>
          <button
            type="button"
            onClick={() => onIgnore(item.emitter_id)}
            disabled={ignoring}
            className="rounded border border-slate-700 px-3 py-1 text-xs text-slate-300 transition hover:border-slate-500 hover:text-slate-100 disabled:opacity-50"
          >
            Ignore
          </button>
        </div>
      </div>
      {expanded && (
        <div className="mt-3">
          <CoTravelDetails emitterId={item.emitter_id} from={from} to={to} />
        </div>
      )}
    </li>
  );
}

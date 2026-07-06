// Full-width, one-line search input (Phase 2 of the list-pages UX cleanup) —
// shared by Emissions now, Emitters/Entities in Phases 3/4. Debounces its
// own keystrokes before calling `onChange` (via the existing
// `useDebouncedValue` hook, same debounce primitive `RuleBuilder`'s preview
// query already uses) so a fast typist doesn't fire a network request per
// keystroke; the input itself stays instantly responsive (local `draft`
// state), only the outward `onChange` call lags.
import { useEffect, useState } from 'react';
import { useDebouncedValue } from '../hooks/useDebouncedValue';

export interface SearchBarProps {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
}

/** How long to wait after the last keystroke before calling `onChange`. */
const SEARCH_DEBOUNCE_MS = 300;

export default function SearchBar({ value, onChange, placeholder }: SearchBarProps) {
  const [draft, setDraft] = useState(value);
  const debouncedDraft = useDebouncedValue(draft, SEARCH_DEBOUNCE_MS);

  // A caller-driven `value` change (e.g. a "clear filters" action
  // elsewhere) re-syncs the local draft so the input reflects it
  // immediately rather than waiting out the debounce.
  useEffect(() => {
    setDraft(value);
  }, [value]);

  useEffect(() => {
    if (debouncedDraft !== value) onChange(debouncedDraft);
    // Intentionally omits `value`/`onChange` from deps: this effect should
    // only fire when the *debounced draft* settles on something new, not
    // when the caller's own `value` changes (that direction is handled by
    // the sync effect above, and re-running this one on `value` changes
    // would re-emit an `onChange` echo of what the caller just set).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debouncedDraft]);

  return (
    <div className="relative w-full">
      <span className="pointer-events-none absolute inset-y-0 left-0 flex items-center pl-2.5 text-slate-500">
        <svg
          aria-hidden="true"
          viewBox="0 0 20 20"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.5}
          className="h-4 w-4"
        >
          <circle cx="9" cy="9" r="6" />
          <path d="M17 17l-4-4" strokeLinecap="round" />
        </svg>
      </span>
      <input
        type="search"
        aria-label="Search"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        placeholder={placeholder ?? 'Search…'}
        className="w-full rounded border border-slate-700 bg-slate-950 py-1.5 pl-8 pr-3 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
      />
    </div>
  );
}

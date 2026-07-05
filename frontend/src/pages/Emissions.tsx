// Task 9.4: browse/filter captured emissions and assign a batch of them to
// an emitter.
//
// Filtering reuses Task 9.2's `FilterBar` (kind="wifi" — the only capture
// kind this schema currently supports, see backend
// `fluxfang_core::catalog::catalog_for`) — its `FilterState` is translated
// to `GET /api/emissions` query params via `filterToQueryParams`, with this
// page's own `limit`/`offset` pagination params appended on top (per Task
// 9.2's hand-off note: FilterBar deliberately stops at
// field-condition/text/unassigned filters).
//
// The query is keyed off `queryKeys.emissions` (with the serialized params
// appended) so `useLiveEvents` (Task 9.1) invalidating that key on every WS
// `emission` frame refetches this page's current filter/page automatically
// — `invalidateQueries` matches by prefix.
//
// "Assign to emitter": row checkboxes select a batch, and "Assign to
// emitter" opens a modal with a `RuleBuilder` (Task 9.2, showPreview) whose
// initial rule is prefilled as `bssid eq <first selected emission's
// payload.bssid>` — the same default rule the backend itself would build
// from `from_emission_id` (see `fluxfang-api::emitters`'s
// `resolve_match_criteria`), just built client-side so it's visible/editable
// in the modal before submitting. Submitting calls `POST /api/emitters` with
// `{name, type, match_criteria: <rule>}` and surfaces the returned
// `attached_count`.
import { useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import type { Emission } from '../api/emissions';
import { listEmissions } from '../api/emissions';
import { createEmitter, listEmitters } from '../api/emitters';
import FilterBar from '../components/FilterBar';
import { EMPTY_FILTER_STATE, filterToQueryParams } from '../components/filterState';
import type { FilterState } from '../components/filterState';
import RuleBuilder from '../components/RuleBuilder';
import type { Rule } from '../types/rule';

const PAGE_SIZE_OPTIONS = [25, 50, 100, 200] as const;
const DEFAULT_LIMIT: (typeof PAGE_SIZE_OPTIONS)[number] = 50;

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';

function formatObservedAt(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** Reads a payload key defensively — `payload`'s shape depends on `kind`
 * (see `Emission`'s doc comment), so any of these may be absent. */
function payloadText(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === 'string' || typeof value === 'number' ? String(value) : '—';
}

function locationText(emission: Emission): string {
  if (emission.lat === null || emission.lon === null) return '—';
  return `${emission.lat.toFixed(5)}, ${emission.lon.toFixed(5)}`;
}

/** The default rule a fresh "assign to emitter" modal opens with: `bssid eq
 * <emission's payload.bssid>` — mirrors the backend's own
 * `from_emission_id` default-rule derivation (see module doc comment), just
 * computed client-side. Falls back to an empty rule (user picks fields
 * manually via `RuleBuilder`) if the emission has no string `bssid` in its
 * payload. */
function defaultRuleFor(emission: Emission): Rule {
  const bssid = emission.payload.bssid;
  if (typeof bssid === 'string' && bssid.length > 0) {
    return { match: 'all', conditions: [{ field: 'bssid', op: 'eq', value: bssid }] };
  }
  return { match: 'all', conditions: [] };
}

interface AssignModalProps {
  /** The emission the default rule is derived from — list order's first
   * selected row, not necessarily click order. */
  seedEmission: Emission;
  selectedCount: number;
  onCancel: () => void;
  onAssigned: (attachedCount: number) => void;
}

function AssignModal({ seedEmission, selectedCount, onCancel, onAssigned }: AssignModalProps) {
  const [name, setName] = useState('');
  const [type, setType] = useState('');
  const [rule, setRule] = useState<Rule>(() => defaultRuleFor(seedEmission));

  const createMutation = useMutation({
    mutationFn: createEmitter,
    onSuccess: (result) => onAssigned(result.attached_count),
  });

  const errorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create emitter.'
        : null;

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedType = type.trim();
    createMutation.mutate({
      name: name.trim(),
      type: trimmedType.length > 0 ? trimmedType : undefined,
      match_criteria: rule,
    });
  }

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-lg space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">
          Assign {selectedCount} emission{selectedCount === 1 ? '' : 's'} to emitter
        </h2>

        <div className="space-y-1">
          <label htmlFor="emitter-name" className={labelClassName}>
            Name
          </label>
          <input
            id="emitter-name"
            type="text"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="emitter-type" className={labelClassName}>
            Type (optional)
          </label>
          <input
            id="emitter-type"
            type="text"
            value={type}
            onChange={(event) => setType(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <span className={labelClassName}>Match rule</span>
          <RuleBuilder kind="wifi" value={rule} onChange={setRule} showPreview />
        </div>

        {errorMessage && (
          <p role="alert" className="text-sm text-red-400">
            {errorMessage}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createMutation.isPending || name.trim().length === 0}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {createMutation.isPending ? 'Assigning…' : 'Assign'}
          </button>
        </div>
      </form>
    </div>
  );
}

export default function Emissions() {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<FilterState>(EMPTY_FILTER_STATE);
  const [limit, setLimit] = useState<number>(DEFAULT_LIMIT);
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [showAssignModal, setShowAssignModal] = useState(false);
  const [assignedMessage, setAssignedMessage] = useState<string | null>(null);

  const queryParams = useMemo(() => {
    const params = filterToQueryParams(filter);
    params.set('limit', String(limit));
    params.set('offset', String(offset));
    return params;
  }, [filter, limit, offset]);

  const emissionsQuery = useQuery({
    queryKey: [...queryKeys.emissions, queryParams.toString()],
    queryFn: () => listEmissions(queryParams),
  });

  // Resolves an emission's `emitter_id` to a display name. Not itself
  // invalidated by `useLiveEvents` (emitters aren't touched by a plain
  // emission frame), but this page's own "assign to emitter" mutation
  // invalidates it below, and `queryKeys.emitters` is still the correct key
  // to key this query off per the registry.
  const emittersQuery = useQuery({ queryKey: queryKeys.emitters, queryFn: listEmitters });

  const emitterNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const emitter of emittersQuery.data ?? []) map.set(emitter.id, emitter.name);
    return map;
  }, [emittersQuery.data]);

  function handleFilterChange(next: FilterState): void {
    setFilter(next);
    setOffset(0);
    setSelected(new Set());
  }

  // Pagination (page size or prev/next) changes which emissions are in
  // `items`, so a `selected` id from the previous page may no longer be
  // present — clear it here (mirroring `handleFilterChange`'s reset) so
  // "Assign to emitter (N)" can never reference a `seedEmission` that isn't
  // on the current page (see module doc comment / AssignModal's render
  // guard `showAssignModal && seedEmission`).
  function handleOffsetChange(next: number): void {
    setOffset(next);
    setSelected(new Set());
  }

  function handlePageSizeChange(next: number): void {
    setLimit(next);
    setOffset(0);
    setSelected(new Set());
  }

  function toggleSelected(id: string): void {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  const items = emissionsQuery.data?.items ?? [];
  const total = emissionsQuery.data?.total ?? 0;
  // List order's first selected row — deterministic regardless of click
  // order — is what the modal's default rule is derived from.
  const seedEmission = items.find((emission) => selected.has(emission.id));

  function handleAssigned(attachedCount: number): void {
    setShowAssignModal(false);
    setSelected(new Set());
    setAssignedMessage(`Assigned ${attachedCount} emission${attachedCount === 1 ? '' : 's'}.`);
    void queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
    void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
  }

  const pageStart = total === 0 ? 0 : offset + 1;
  const pageEnd = Math.min(offset + limit, total);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Emissions</h1>
        {assignedMessage && (
          <p role="status" className="text-sm text-amber-400">
            {assignedMessage}
          </p>
        )}
      </div>

      <FilterBar kind="wifi" value={filter} onChange={handleFilterChange} />

      <div className="flex items-center justify-between">
        <button
          type="button"
          disabled={selected.size === 0}
          onClick={() => setShowAssignModal(true)}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Assign to emitter ({selected.size})
        </button>
        <p data-testid="emissions-total" className="text-sm text-slate-400">
          {total} emission{total === 1 ? '' : 's'}
        </p>
      </div>

      {emissionsQuery.isLoading && <p className="text-sm text-slate-500">Loading emissions…</p>}
      {emissionsQuery.isError && <p className="text-sm text-red-400">Failed to load emissions.</p>}

      {items.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-2 font-medium" />
              <th className="py-2 pr-4 font-medium">Observed At</th>
              <th className="py-2 pr-4 font-medium">Kind</th>
              <th className="py-2 pr-4 font-medium">BSSID</th>
              <th className="py-2 pr-4 font-medium">SSID</th>
              <th className="py-2 pr-4 font-medium">Channel</th>
              <th className="py-2 pr-4 font-medium">RSSI</th>
              <th className="py-2 pr-4 font-medium">Emitter</th>
              <th className="py-2 pr-4 font-medium">Location</th>
            </tr>
          </thead>
          <tbody>
            {items.map((emission) => (
              <tr
                key={emission.id}
                data-testid={`emission-row-${emission.id}`}
                className="border-b border-slate-900 align-top"
              >
                <td className="py-2 pr-2">
                  <input
                    type="checkbox"
                    aria-label={`Select emission ${emission.id}`}
                    checked={selected.has(emission.id)}
                    onChange={() => toggleSelected(emission.id)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                </td>
                <td className="py-2 pr-4 text-slate-300">{formatObservedAt(emission.observed_at)}</td>
                <td className="py-2 pr-4 capitalize text-slate-300">{emission.kind}</td>
                <td className="py-2 pr-4 font-mono text-slate-300">{payloadText(emission.payload, 'bssid')}</td>
                <td className="py-2 pr-4 text-slate-300">{payloadText(emission.payload, 'ssid')}</td>
                <td className="py-2 pr-4 text-slate-300">{payloadText(emission.payload, 'channel')}</td>
                <td className="py-2 pr-4 font-mono text-slate-300">
                  {emission.signal_strength ?? '—'}
                </td>
                <td className="py-2 pr-4 text-slate-300">
                  {emission.emitter_id ? (emitterNameById.get(emission.emitter_id) ?? '—') : '—'}
                </td>
                <td className="py-2 pr-4 text-slate-300">{locationText(emission)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {emissionsQuery.data && items.length === 0 && (
        <p className="text-sm text-slate-500">No emissions match this filter.</p>
      )}

      <div className="flex items-center justify-between text-sm text-slate-400">
        <div className="flex items-center gap-2">
          <label htmlFor="emissions-page-size" className={labelClassName}>
            Page size
          </label>
          <select
            id="emissions-page-size"
            value={limit}
            onChange={(event) => {
              handlePageSizeChange(Number(event.target.value));
            }}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
          >
            {PAGE_SIZE_OPTIONS.map((size) => (
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
            onClick={() => handleOffsetChange(Math.max(0, offset - limit))}
            className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Prev
          </button>
          <button
            type="button"
            disabled={offset + limit >= total}
            onClick={() => handleOffsetChange(offset + limit)}
            className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Next
          </button>
        </div>
      </div>

      {showAssignModal && seedEmission && (
        <AssignModal
          seedEmission={seedEmission}
          selectedCount={selected.size}
          onCancel={() => setShowAssignModal(false)}
          onAssigned={handleAssigned}
        />
      )}
    </div>
  );
}

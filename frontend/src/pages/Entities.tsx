// Task 9.6: manage tracked entities (a real-world subject an operator has
// grouped one or more emitters under, e.g. "Bob's phone"). Redesigned per
// the list-pages UX cleanup (Task 4, same convention as `Emitters.tsx`):
// each row's name links to its own deep-linkable detail page
// (`/entities/:id`, `pages/EntityDetailPage.tsx`), which now owns the
// associated-emitters table, aggregate last-seen/live status, detection
// heatmap, name/notes editing, delete, and per-entity alert rules that used
// to live in an inline expand-in-place dropdown here.
import { useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { bulkDeleteEntities, clearEntities, createEntity, listEntities } from '../api/entities';
import type { CreateEntityInput, ListEntitiesParams } from '../api/entities';
import Pagination from '../components/Pagination';
import SearchBar from '../components/SearchBar';
import SelectionToolbar from '../components/SelectionToolbar';
import { useRowSelection } from '../hooks/useRowSelection';

const DEFAULT_LIMIT = 50;

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';
const cancelButtonClassName =
  'rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100';
const submitButtonClassName =
  'rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50';

function formatTimestamp(iso: string | null): string {
  if (!iso) return '—';
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

interface AddEntityFormProps {
  onCancel: () => void;
  onSubmit: (input: CreateEntityInput) => void;
  submitting: boolean;
  errorMessage: string | null;
}

function AddEntityForm({ onCancel, onSubmit, submitting, errorMessage }: AddEntityFormProps) {
  const [name, setName] = useState('');
  const [notes, setNotes] = useState('');

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = name.trim();
    if (trimmedName.length === 0) return;
    const trimmedNotes = notes.trim();
    onSubmit({ name: trimmedName, notes: trimmedNotes.length > 0 ? trimmedNotes : undefined });
  }

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-md space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">Add Entity</h2>

        <div className="space-y-1">
          <label htmlFor="entity-name" className={labelClassName}>
            Name
          </label>
          <input
            id="entity-name"
            type="text"
            required
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="entity-notes" className={labelClassName}>
            Notes
          </label>
          <textarea
            id="entity-notes"
            value={notes}
            onChange={(event) => setNotes(event.target.value)}
            className={`${inputClassName} min-h-[4rem]`}
          />
        </div>

        {errorMessage && (
          <p role="alert" className="text-sm text-red-400">
            {errorMessage}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button type="button" onClick={onCancel} className={cancelButtonClassName}>
            Cancel
          </button>
          <button type="submit" disabled={submitting} className={submitButtonClassName}>
            {submitting ? 'Adding…' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
}

export default function Entities() {
  const queryClient = useQueryClient();
  const [showAddForm, setShowAddForm] = useState(false);
  const [q, setQ] = useState('');
  const [limit, setLimit] = useState(DEFAULT_LIMIT);
  const [offset, setOffset] = useState(0);

  const queryParams = useMemo<ListEntitiesParams>(() => {
    const params: ListEntitiesParams = { limit, offset };
    const trimmedQ = q.trim();
    if (trimmedQ.length > 0) params.search = trimmedQ;
    return params;
  }, [q, limit, offset]);

  const entitiesQuery = useQuery({
    queryKey: [...queryKeys.entities, JSON.stringify(queryParams)],
    queryFn: () => listEntities(queryParams),
  });

  const entities = entitiesQuery.data?.items ?? [];
  const total = entitiesQuery.data?.total ?? 0;
  const itemIds = entities.map((entity) => entity.id);
  const selection = useRowSelection(itemIds);

  function invalidateEntities(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
  }

  function resetToFirstPage(): void {
    setOffset(0);
    selection.clear();
  }

  function handleSearchChange(next: string): void {
    setQ(next);
    resetToFirstPage();
  }

  function handlePaginationChange(nextLimit: number, nextOffset: number): void {
    setLimit(nextLimit);
    setOffset(nextOffset);
    selection.clear();
  }

  const createMutation = useMutation({
    mutationFn: (input: CreateEntityInput) => createEntity(input),
    onSuccess: () => {
      invalidateEntities();
      setShowAddForm(false);
    },
  });

  const createErrorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create entity.'
        : null;

  const bulkDeleteMutation = useMutation({
    mutationFn: bulkDeleteEntities,
    onSuccess: () => {
      selection.clear();
      invalidateEntities();
    },
  });

  const clearAllMutation = useMutation({
    mutationFn: clearEntities,
    onSuccess: () => {
      selection.clear();
      invalidateEntities();
    },
  });

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Entities</h1>
        <button
          type="button"
          onClick={() => setShowAddForm(true)}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400"
        >
          Add Entity
        </button>
      </div>

      <SearchBar value={q} onChange={handleSearchChange} placeholder="Search entities…" />

      <div className="flex items-center justify-between">
        <SelectionToolbar
          selectedCount={selection.selected.size}
          onDeleteSelected={() => bulkDeleteMutation.mutate(Array.from(selection.selected))}
          onClearAll={() => clearAllMutation.mutate()}
          itemLabelPlural="Entities"
        />
        <p data-testid="entities-total" className="text-sm text-slate-400">
          {total} entit{total === 1 ? 'y' : 'ies'}
        </p>
      </div>

      {entitiesQuery.isLoading && <p className="text-sm text-slate-500">Loading entities…</p>}
      {entitiesQuery.isError && <p className="text-sm text-red-400">Failed to load entities.</p>}
      {entitiesQuery.data && entities.length === 0 && (
        <p className="text-sm text-slate-500">No entities match this filter.</p>
      )}

      {entities.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-2 font-medium">
                <input
                  type="checkbox"
                  aria-label="Select all entities on this page"
                  checked={selection.allSelected}
                  onChange={() => selection.toggleAll(itemIds)}
                  className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                />
              </th>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Notes</th>
              <th className="py-2 pr-4 font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {entities.map((entity) => (
              <tr key={entity.id} data-testid={`entity-row-${entity.id}`} className="border-b border-slate-900 align-top">
                <td className="py-2 pr-2">
                  <input
                    type="checkbox"
                    aria-label={`Select entity ${entity.id}`}
                    checked={selection.selected.has(entity.id)}
                    onChange={() => selection.toggle(entity.id)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                </td>
                <td className="py-2 pr-4 text-slate-200">
                  <Link
                    to={`/entities/${entity.id}`}
                    className="underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                  >
                    {entity.name}
                  </Link>
                </td>
                <td className="py-2 pr-4 text-slate-300">{entity.notes ?? '—'}</td>
                <td className="py-2 pr-4 text-slate-300">{formatTimestamp(entity.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <Pagination total={total} limit={limit} offset={offset} onChange={handlePaginationChange} />

      {showAddForm && (
        <AddEntityForm
          onCancel={() => {
            setShowAddForm(false);
            createMutation.reset();
          }}
          onSubmit={(input) => createMutation.mutate(input)}
          submitting={createMutation.isPending}
          errorMessage={createErrorMessage}
        />
      )}
    </div>
  );
}

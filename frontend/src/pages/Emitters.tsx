// Task 9.5, redesigned per Phase 3 of the list-pages UX cleanup (see
// `docs/superpowers/specs/2026-07-05-list-pages-ux-cleanup-design.md`,
// "Emitters page" section): browse discovered emitters, search/filter them,
// and manage each one's entity grouping — with compact one-row rows. Each
// row's name links to its own deep-linkable detail page
// (`/emitters/:id`, `pages/EmitterDetailPage.tsx`), which now owns the match
// rule editor, full attributes dump, and detection heatmap that used to live
// in an inline expand-in-place dropdown here.
//
// Top controls: a full-width `SearchBar` (`q` -> `search`) plus an Entity
// filter `<select>` (-> `entity_id`) alongside it. Both feed one
// `GET /api/emitters` query (search/entity_id/limit/offset), keyed off
// `queryKeys.emitters` so `useLiveEvents` invalidating that key refetches
// this page's current filter/page — same convention as `Emissions.tsx`.
// Changing search or the entity filter resets `offset` to 0 and clears the
// row selection.
//
// "Associate to entity" folds the old separate "New Entity" button into a
// single per-row `<select>`: each existing entity, a "+ New entity…" item
// (prompts for a name via `window.prompt`, then `POST /api/entities`
// followed by `PATCH /api/emitters/:id { entity_id }` — see
// `createAndAssociateMutation` below for why this two-call sequence is used
// instead of `POST /api/emitters/with-entity`), and a "Detach" item when the
// row is already associated.
//
// Mass-select ("Delete selected"/"Clear All Emitters") uses the shared
// `useRowSelection`/`SelectionToolbar` (Phase 2) against
// `bulkDeleteEmitters`/`clearEmitters`; `Pagination` (Phase 2) drives
// `limit`/`offset`.
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { queryKeys } from "../api/queryKeys";
import { createEntity, listEntities } from "../api/entities";
import type { Entity } from "../api/entities";
import {
  bulkDeleteEmitters,
  clearEmitters,
  deleteEmitter,
  listEmitters,
  patchEmitter,
} from "../api/emitters";
import type { Emitter, ListEmittersParams } from "../api/emitters";
import Pagination from "../components/Pagination";
import SearchBar from "../components/SearchBar";
import SelectionToolbar from "../components/SelectionToolbar";
import { useRowSelection } from "../hooks/useRowSelection";
import {
  MacIdentityCell,
  TypeBadge,
  formatCompact,
} from "../components/emitterDisplay";

const DEFAULT_LIMIT = 50;

/** Sentinel `<select>` values for the folded "Associate…" control — never a
 * real entity id (those are UUIDs from the backend). */
const NEW_ENTITY_VALUE = "__new_entity__";
const DETACH_VALUE = "__detach__";

/** Sentinel `<select>` value for the Type-filter dropdown's default "All
 * types" option — distinct from any real `emitter_type` key. */
const ALL_TYPES_VALUE = "";

const selectClassName =
  "rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 focus:border-amber-500 focus:outline-none";

export default function Emitters() {
  const queryClient = useQueryClient();
  const [q, setQ] = useState("");
  const [entityId, setEntityId] = useState("");
  const [emitterType, setEmitterType] = useState(ALL_TYPES_VALUE);
  const [limit, setLimit] = useState(DEFAULT_LIMIT);
  const [offset, setOffset] = useState(0);

  const queryParams = useMemo<ListEmittersParams>(() => {
    const params: ListEmittersParams = { limit, offset };
    const trimmedQ = q.trim();
    if (trimmedQ.length > 0) params.search = trimmedQ;
    if (entityId.length > 0) params.entity_id = entityId;
    if (emitterType.length > 0) params.emitter_type = emitterType;
    return params;
  }, [q, entityId, emitterType, limit, offset]);

  const emittersQuery = useQuery({
    queryKey: [...queryKeys.emitters, JSON.stringify(queryParams)],
    queryFn: () => listEmitters(queryParams),
  });

  // Interim `{limit: 500}` cap for the entity lookups (filter dropdown +
  // associate dropdown + resolving each row's entity name) — `GET
  // /api/entities` returns a paginated `{items, total}` envelope, but this
  // page has no reason to paginate that list itself; 500 keeps "every
  // entity available to pick from" without adding a second pagination UI.
  const entitiesQuery = useQuery({
    queryKey: queryKeys.entities,
    queryFn: () => listEntities({ limit: 500 }),
  });
  const entities = useMemo(
    () => entitiesQuery.data?.items ?? [],
    [entitiesQuery.data],
  );

  const entityNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const entity of entities) map.set(entity.id, entity.name);
    return map;
  }, [entities]);

  const emitters = emittersQuery.data?.items ?? [];
  const total = emittersQuery.data?.total ?? 0;
  const itemIds = emitters.map((emitter) => emitter.id);
  const selection = useRowSelection(itemIds);

  // Type-filter dropdown options — distinct non-null `emitter_type` keys from
  // the current result set (paired with each type's human `type_label`),
  // same "derive from what's loaded" approach `MapView` uses for its category
  // layers. Server-side filtering means once a type is picked the result set
  // is all that one type, so it stays present; clearing back to "All types"
  // restores the full spread on the next unfiltered fetch.
  const typeOptions = useMemo(() => {
    const byKey = new Map<string, string>();
    for (const emitter of emitters) {
      if (emitter.emitter_type)
        byKey.set(
          emitter.emitter_type,
          emitter.type_label ?? emitter.emitter_type,
        );
    }
    return Array.from(byKey, ([key, label]) => ({ key, label })).sort((a, b) =>
      a.label.localeCompare(b.label),
    );
  }, [emitters]);

  function invalidateEmitters(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
  }

  function resetToFirstPage(): void {
    setOffset(0);
    selection.clear();
  }

  function handleSearchChange(next: string): void {
    setQ(next);
    resetToFirstPage();
  }

  function handleEntityFilterChange(next: string): void {
    setEntityId(next);
    resetToFirstPage();
  }

  function handleTypeFilterChange(next: string): void {
    setEmitterType(next);
    resetToFirstPage();
  }

  function handlePaginationChange(nextLimit: number, nextOffset: number): void {
    setLimit(nextLimit);
    setOffset(nextOffset);
    selection.clear();
  }

  // Associate-to-existing, detach (`{ entity_id: null }`), the rule
  // enable/disable toggle, and the manual randomized-MAC override all go
  // through this one mutation — each is the same `PATCH /api/emitters/:id`
  // call with a different body.
  const patchMutation = useMutation({
    mutationFn: ({
      id,
      body,
    }: {
      id: string;
      body: Parameters<typeof patchEmitter>[1];
    }) => patchEmitter(id, body),
    onSuccess: invalidateEmitters,
  });

  // "+ New entity…" (folded into the Associate select, design doc's
  // Emitters section): two sequential calls against an *existing* emitter —
  // `POST /api/entities` then `PATCH /api/emitters/:id { entity_id }` —
  // rather than `POST /api/emitters/with-entity` (that endpoint creates a
  // *brand-new* emitter row atomically with the entity, which is wrong here
  // since every row on this page is an already-existing emitter whose id,
  // `match_criteria`, and emission history must survive the association
  // unchanged). Invalidates both `queryKeys.entities` (the new entity needs
  // to show up in every row's dropdown) and `queryKeys.emitters` (this
  // row's own entity column).
  const createAndAssociateMutation = useMutation({
    mutationFn: async ({
      emitterId,
      entityName,
    }: {
      emitterId: string;
      entityName: string;
    }) => {
      const entity = await createEntity({ name: entityName });
      const emitter = await patchEmitter(emitterId, { entity_id: entity.id });
      return { entity, emitter };
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
      invalidateEmitters();
    },
  });

  const deleteMutation = useMutation({
    mutationFn: deleteEmitter,
    onSuccess: invalidateEmitters,
  });

  const bulkDeleteMutation = useMutation({
    mutationFn: bulkDeleteEmitters,
    onSuccess: () => {
      selection.clear();
      invalidateEmitters();
    },
  });

  const clearAllMutation = useMutation({
    mutationFn: clearEmitters,
    onSuccess: () => {
      selection.clear();
      invalidateEmitters();
    },
  });

  function handleAssociateSelect(emitter: Emitter, value: string): void {
    if (value === NEW_ENTITY_VALUE) {
      const name = window.prompt("New entity name?");
      const trimmed = name?.trim() ?? "";
      if (trimmed.length === 0) return;
      createAndAssociateMutation.mutate({
        emitterId: emitter.id,
        entityName: trimmed,
      });
      return;
    }
    if (value === DETACH_VALUE) {
      patchMutation.mutate({ id: emitter.id, body: { entity_id: null } });
      return;
    }
    if (value.length === 0) return;
    patchMutation.mutate({ id: emitter.id, body: { entity_id: value } });
  }

  function handleDelete(emitterId: string): void {
    if (!window.confirm("Delete this emitter?")) return;
    deleteMutation.mutate(emitterId);
  }

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold text-slate-100">Emitters</h1>

      <div className="flex items-center gap-3">
        <SearchBar
          value={q}
          onChange={handleSearchChange}
          placeholder="Search emitters…"
        />
        <label htmlFor="emitters-type-filter" className="sr-only">
          Filter by type
        </label>
        <select
          id="emitters-type-filter"
          aria-label="Filter by type"
          value={emitterType}
          onChange={(event) => handleTypeFilterChange(event.target.value)}
          className={selectClassName}
        >
          <option value={ALL_TYPES_VALUE}>All types</option>
          {typeOptions.map((option) => (
            <option key={option.key} value={option.key}>
              {option.label}
            </option>
          ))}
        </select>
        <label htmlFor="emitters-entity-filter" className="sr-only">
          Filter by entity
        </label>
        <select
          id="emitters-entity-filter"
          aria-label="Filter by entity"
          value={entityId}
          onChange={(event) => handleEntityFilterChange(event.target.value)}
          className={selectClassName}
        >
          <option value="">All entities</option>
          {entities.map((entity: Entity) => (
            <option key={entity.id} value={entity.id}>
              {entity.name}
            </option>
          ))}
        </select>
      </div>

      <div className="flex items-center justify-between">
        <SelectionToolbar
          selectedCount={selection.selected.size}
          onDeleteSelected={() =>
            bulkDeleteMutation.mutate(Array.from(selection.selected))
          }
          onClearAll={() => clearAllMutation.mutate()}
          itemLabelPlural="Emitters"
        />
        <p data-testid="emitters-total" className="text-sm text-slate-400">
          {total} emitter{total === 1 ? "" : "s"}
        </p>
      </div>

      {emittersQuery.isLoading && (
        <p className="text-sm text-slate-500">Loading emitters…</p>
      )}
      {emittersQuery.isError && (
        <p className="text-sm text-red-400">Failed to load emitters.</p>
      )}
      {emittersQuery.data && emitters.length === 0 && (
        <p className="text-sm text-slate-500">No emitters match this filter.</p>
      )}

      {emitters.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-2 font-medium">
                <input
                  type="checkbox"
                  aria-label="Select all emitters on this page"
                  checked={selection.allSelected}
                  onChange={() => selection.toggleAll(itemIds)}
                  className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                />
              </th>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Type</th>
              <th className="py-2 pr-4 font-medium">MAC/Identity</th>
              <th className="py-2 pr-4 font-medium">First Seen</th>
              <th className="py-2 pr-4 font-medium">Last Seen</th>
              <th className="py-2 pr-4 font-medium">Entity</th>
              <th className="py-2 pr-2 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {emitters.map((emitter) => {
              const entityName = emitter.entity_id
                ? (entityNameById.get(emitter.entity_id) ?? "—")
                : "—";
              const rowBusy =
                (patchMutation.isPending &&
                  patchMutation.variables?.id === emitter.id) ||
                (deleteMutation.isPending &&
                  deleteMutation.variables === emitter.id) ||
                (createAndAssociateMutation.isPending &&
                  createAndAssociateMutation.variables?.emitterId ===
                    emitter.id);

              return (
                <tr
                  key={emitter.id}
                  data-testid={`emitter-row-${emitter.id}`}
                  className="border-b border-slate-900"
                >
                  <td className="py-2 pr-2">
                    <input
                      type="checkbox"
                      aria-label={`Select emitter ${emitter.id}`}
                      checked={selection.selected.has(emitter.id)}
                      onChange={() => selection.toggle(emitter.id)}
                      className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                    />
                  </td>
                  <td className="py-2 pr-4 text-slate-200">
                    <Link
                      to={`/emitters/${emitter.id}`}
                      className="text-slate-200 underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                    >
                      {emitter.name}
                    </Link>
                  </td>
                  <td className="py-2 pr-4">
                    <TypeBadge emitter={emitter} />
                  </td>
                  <td className="py-2 pr-4">
                    <MacIdentityCell emitter={emitter} />
                  </td>
                  <td className="py-2 pr-4 whitespace-nowrap text-slate-300">
                    {formatCompact(emitter.first_seen_at)}
                  </td>
                  <td className="py-2 pr-4 whitespace-nowrap text-slate-300">
                    {formatCompact(emitter.last_seen_at)}
                  </td>
                  <td
                    data-testid={`emitter-entity-${emitter.id}`}
                    className="py-2 pr-4 text-slate-300"
                  >
                    {entityName}
                  </td>
                  <td className="py-2 pr-2">
                    <div className="flex items-center gap-1.5 whitespace-nowrap">
                      <select
                        aria-label={`Associate ${emitter.name} to an entity`}
                        value=""
                        disabled={rowBusy}
                        onChange={(event) =>
                          handleAssociateSelect(emitter, event.target.value)
                        }
                        className={selectClassName}
                      >
                        <option value="" disabled>
                          Associate…
                        </option>
                        {emitter.entity_id && (
                          <option value={DETACH_VALUE}>Detach</option>
                        )}
                        {entities.map((entity: Entity) => (
                          <option key={entity.id} value={entity.id}>
                            {entity.name}
                          </option>
                        ))}
                        <option value={NEW_ENTITY_VALUE}>+ New entity…</option>
                      </select>

                      <button
                        type="button"
                        disabled={rowBusy}
                        onClick={() => handleDelete(emitter.id)}
                        className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {deleteMutation.isPending &&
                        deleteMutation.variables === emitter.id
                          ? "Deleting…"
                          : "Delete"}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <Pagination
        total={total}
        limit={limit}
        offset={offset}
        onChange={handlePaginationChange}
      />
    </div>
  );
}

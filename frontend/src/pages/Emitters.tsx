// Task 9.5, redesigned per Phase 3 of the list-pages UX cleanup (see
// `docs/superpowers/specs/2026-07-05-list-pages-ux-cleanup-design.md`,
// "Emitters page" section): browse discovered emitters, search/filter them,
// and manage each one's entity grouping — with compact one-row rows (an
// expand toggle reveals the match rule, full attributes, and the detection
// heatmap) instead of the old always-expanded multi-line layout.
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
import { Fragment, useMemo, useState } from "react";
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
  setEmitterRule,
} from "../api/emitters";
import type { Emitter, ListEmittersParams } from "../api/emitters";
import { listEmissions } from "../api/emissions";
import type { Emission } from "../api/emissions";
import type { Rule } from "../types/rule";
import EmissionsHeatmap from "../components/EmissionsHeatmap";
import RuleBuilder from "../components/RuleBuilder";
import type { HeatmapPoint } from "../components/mapData";
import Pagination from "../components/Pagination";
import SearchBar from "../components/SearchBar";
import SelectionToolbar from "../components/SelectionToolbar";
import { useRowSelection } from "../hooks/useRowSelection";
import {
  EMPTY_RULE,
  MacIdentityCell,
  RULE_EDITOR_KIND,
  TypeBadge,
  asRule,
  formatAttributeValue,
  formatCompact,
  formatTimestamp,
  isRandomizedMac,
  ruleConditions,
  ruleMatchModeLabel,
} from "../components/emitterDisplay";

const DEFAULT_LIMIT = 50;
const DETAIL_EMISSIONS_LIMIT = 20;
// Task C (emitter auto-classification design doc): a separate, larger-limit
// fetch backs the detection heatmap — the "Recent emissions" table only
// needs its own last-20 window, but "everywhere this emitter has been
// heard" wants a much wider sample, same 500 cap `MapView.tsx`'s overview
// heatmap uses.
const HEATMAP_EMISSIONS_LIMIT = 500;
// [checkbox] Name, Type, MAC/Identity, First seen, Last seen, Entity, Actions.
const TABLE_COLUMN_COUNT = 8;

/** Sentinel `<select>` values for the folded "Associate…" control — never a
 * real entity id (those are UUIDs from the backend). */
const NEW_ENTITY_VALUE = "__new_entity__";
const DETACH_VALUE = "__detach__";

/** Sentinel `<select>` value for the Type-filter dropdown's default "All
 * types" option — distinct from any real `emitter_type` key. */
const ALL_TYPES_VALUE = "";

const selectClassName =
  "rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 focus:border-amber-500 focus:outline-none";

interface EmitterDetailProps {
  emitter: Emitter;
  rowBusy: boolean;
  onToggleMatchEnabled: () => void;
  onToggleRandomized: () => void;
}

/** The expanded row's content (design doc's "Expand" section): the
 * emitter's full `attributes`, its match rule (with the enable/disable
 * toggle — moved here from the collapsed row), the manual randomized-MAC
 * override, and the detection heatmap/recent emissions
 * (`GET /api/emissions?emitter_id=:id`). Its own component (rather than
 * inline in the parent) so the recent-emissions/heatmap queries only ever
 * run while the row is actually expanded — they unmount (and the queries go
 * away) when the row collapses. */
function EmitterDetail({
  emitter,
  rowBusy,
  onToggleMatchEnabled,
  onToggleRandomized,
}: EmitterDetailProps) {
  const queryClient = useQueryClient();
  const params = useMemo(
    () => ({ emitter_id: emitter.id, limit: DETAIL_EMISSIONS_LIMIT }),
    [emitter.id],
  );

  // Local draft of the rule being edited, seeded from the emitter's current
  // `match_criteria` (or an empty ALL rule when it has none). Fully
  // controlled by `RuleBuilder` from here — only committed to the backend on
  // "Save rule". Keyed by emitter id via the initializer; since only one row
  // is expanded at a time this component unmounts/remounts per emitter, so
  // the seed always reflects the row just opened.
  const [draftRule, setDraftRule] = useState<Rule>(
    () => asRule(emitter.match_criteria) ?? EMPTY_RULE,
  );

  // `POST /api/emitters/:id/rule` — replace the rule, then re-attach every
  // already-stored matching emission (the returned `attached_count`).
  // Invalidates `queryKeys.emitters` so this row (and its entity/rule
  // columns) refetch, same convention as the parent's `patchMutation`.
  const saveRuleMutation = useMutation({
    mutationFn: (rule: Rule) => setEmitterRule(emitter.id, rule),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
    },
  });

  // Keyed off `queryKeys.emissions` (per the registry) so a live `emission`
  // WS frame invalidating that key refetches this too, same convention as
  // `Emissions.tsx`'s own filtered query.
  const recentQuery = useQuery({
    queryKey: [...queryKeys.emissions, "emitter-detail", emitter.id],
    queryFn: () => {
      const p = new URLSearchParams();
      p.set("emitter_id", params.emitter_id);
      p.set("limit", String(params.limit));
      return listEmissions(p);
    },
  });

  // Detection heatmap: "where this emitter has been heard" — a wider,
  // separately-fetched sample than the "Recent emissions" table above,
  // filtered client-side to only located rows (an emission with no GPS fix
  // has null `lon`/`lat`).
  const heatmapQuery = useQuery({
    queryKey: [...queryKeys.emissions, "heatmap", emitter.id],
    queryFn: () => {
      const p = new URLSearchParams();
      p.set("emitter_id", emitter.id);
      p.set("limit", String(HEATMAP_EMISSIONS_LIMIT));
      return listEmissions(p);
    },
  });

  const heatmapPoints = useMemo<HeatmapPoint[]>(() => {
    const items = heatmapQuery.data?.items ?? [];
    return items
      .filter(
        (item): item is Emission & { lon: number; lat: number } =>
          item.lon !== null && item.lat !== null,
      )
      .map((item) => ({ lon: item.lon, lat: item.lat }));
  }, [heatmapQuery.data]);

  const conditions = ruleConditions(emitter.match_criteria);
  const items = recentQuery.data?.items ?? [];
  const attributeEntries = Object.entries(emitter.attributes ?? {});

  return (
    <tr
      data-testid={`emitter-detail-${emitter.id}`}
      className="border-b border-slate-900 bg-slate-950/40"
    >
      <td colSpan={TABLE_COLUMN_COUNT} className="px-4 py-3">
        <div className="space-y-4">
          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
              Attributes
            </h3>
            {attributeEntries.length === 0 ? (
              <p className="mt-1 text-sm text-slate-500">
                No attributes recorded.
              </p>
            ) : (
              <dl className="mt-1 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
                {attributeEntries.map(([key, value]) => (
                  <Fragment key={key}>
                    <dt className="text-slate-500">{key}</dt>
                    <dd className="font-mono text-slate-200">
                      {formatAttributeValue(value)}
                    </dd>
                  </Fragment>
                ))}
              </dl>
            )}
            {emitter.emitter_type === "wifi_client" && (
              <button
                type="button"
                disabled={rowBusy}
                onClick={onToggleRandomized}
                className="mt-2 text-xs text-slate-500 underline decoration-dotted hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {isRandomizedMac(emitter.attributes ?? {})
                  ? "Mark as not randomized"
                  : "Mark as randomized"}
              </button>
            )}
          </div>

          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
              Match rule
            </h3>
            <label className="mt-1 flex items-center gap-1.5 text-xs text-slate-400">
              <input
                type="checkbox"
                role="switch"
                aria-label={`Rule enabled for ${emitter.name}`}
                checked={emitter.match_enabled}
                disabled={rowBusy}
                onChange={onToggleMatchEnabled}
                className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
              />
              {emitter.match_enabled ? "Enabled" : "Disabled"}
            </label>
            {!emitter.match_enabled && (
              <p className="mt-1 text-xs text-amber-400">
                Disabled — new matching emissions won&apos;t auto-attach.
              </p>
            )}
            {conditions.length === 0 ? (
              <p className="mt-1 text-sm text-slate-500">
                No conditions — this emitter doesn&apos;t auto-attach new
                emissions.
              </p>
            ) : (
              <div className="mt-1 text-sm text-slate-300">
                <span className="text-slate-500">
                  Match {ruleMatchModeLabel(emitter.match_criteria)} of:
                </span>
                <ul className="mt-1 list-inside list-disc space-y-0.5 font-mono text-slate-200">
                  {conditions.map((text, index) => (
                    <li key={index}>{text}</li>
                  ))}
                </ul>
              </div>
            )}

            <div className="mt-3 space-y-2 border-t border-slate-800 pt-3">
              <h4 className="text-xs font-medium uppercase tracking-wide text-slate-500">
                Edit rule
              </h4>
              <RuleBuilder
                kind={RULE_EDITOR_KIND}
                value={draftRule}
                onChange={setDraftRule}
              />
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  disabled={saveRuleMutation.isPending}
                  onClick={() => saveRuleMutation.mutate(draftRule)}
                  className="rounded border border-amber-600 bg-amber-500/10 px-3 py-1.5 text-sm text-amber-400 transition hover:border-amber-500 hover:bg-amber-500/20 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {saveRuleMutation.isPending ? "Saving…" : "Save rule"}
                </button>
                {saveRuleMutation.isSuccess && (
                  <span className="text-xs text-slate-400">
                    Saved — attached {saveRuleMutation.data.attached_count}{" "}
                    emission
                    {saveRuleMutation.data.attached_count === 1 ? "" : "s"}.
                  </span>
                )}
                {saveRuleMutation.isError && (
                  <span className="text-xs text-red-400">
                    Failed to save rule.
                  </span>
                )}
              </div>
            </div>
          </div>

          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
              Detection heatmap
            </h3>
            <p className="mt-1 text-xs text-slate-500">
              Where this emitter has been heard.
            </p>
            <div className="mt-1">
              <EmissionsHeatmap points={heatmapPoints} />
            </div>
          </div>

          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
              Recent emissions
            </h3>
            {recentQuery.isLoading && (
              <p className="mt-1 text-sm text-slate-500">Loading emissions…</p>
            )}
            {recentQuery.isError && (
              <p className="mt-1 text-sm text-red-400">
                Failed to load recent emissions.
              </p>
            )}
            {recentQuery.data && items.length === 0 && (
              <p className="mt-1 text-sm text-slate-500">
                No emissions recorded for this emitter yet.
              </p>
            )}
            {items.length > 0 && (
              <table className="mt-1 w-full border-collapse text-left text-xs">
                <thead>
                  <tr className="border-b border-slate-800 text-slate-500">
                    <th className="py-1 pr-4 font-medium">Observed At</th>
                    <th className="py-1 pr-4 font-medium">BSSID</th>
                    <th className="py-1 pr-4 font-medium">SSID</th>
                    <th className="py-1 pr-4 font-medium">RSSI</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((emission: Emission) => (
                    <tr
                      key={emission.id}
                      data-testid={`emitter-detail-emission-${emission.id}`}
                    >
                      <td className="py-1 pr-4 text-slate-300">
                        {formatTimestamp(emission.observed_at)}
                      </td>
                      <td className="py-1 pr-4 font-mono text-slate-300">
                        {typeof emission.payload.bssid === "string"
                          ? emission.payload.bssid
                          : "—"}
                      </td>
                      <td className="py-1 pr-4 text-slate-300">
                        {typeof emission.payload.ssid === "string"
                          ? emission.payload.ssid
                          : "—"}
                      </td>
                      <td className="py-1 pr-4 font-mono text-slate-300">
                        {emission.signal_strength ?? "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      </td>
    </tr>
  );
}

export default function Emitters() {
  const queryClient = useQueryClient();
  const [q, setQ] = useState("");
  const [entityId, setEntityId] = useState("");
  const [emitterType, setEmitterType] = useState(ALL_TYPES_VALUE);
  const [limit, setLimit] = useState(DEFAULT_LIMIT);
  const [offset, setOffset] = useState(0);
  const [expandedId, setExpandedId] = useState<string | null>(null);

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

  // The rule enable/disable toggle (moved into the expanded detail panel) —
  // a plain `{ match_enabled }` PATCH, flipping whatever the emitter's
  // current value is.
  function handleToggleMatchEnabled(emitter: Emitter): void {
    patchMutation.mutate({
      id: emitter.id,
      body: { match_enabled: !emitter.match_enabled },
    });
  }

  // Manual randomized-MAC override (moved into the expanded detail panel) —
  // since `PATCH .../attributes` is a full replace, not a merge (see
  // `PatchEmitterInput`'s doc comment in `api/emitters.ts`), this reads the
  // emitter's *already-loaded* `attributes` (no extra GET needed — it's the
  // same object this row is rendering from `queryKeys.emitters`), spreads
  // it, and flips just `randomized_mac` before sending the whole thing.
  function handleToggleRandomized(emitter: Emitter): void {
    const currentAttributes = emitter.attributes ?? {};
    const currentlyRandomized = isRandomizedMac(currentAttributes);
    patchMutation.mutate({
      id: emitter.id,
      body: {
        attributes: {
          ...currentAttributes,
          randomized_mac: !currentlyRandomized,
        },
      },
    });
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
              const isExpanded = expandedId === emitter.id;
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

              function toggleExpanded(): void {
                setExpandedId(isExpanded ? null : emitter.id);
              }

              return (
                <Fragment key={emitter.id}>
                  <tr
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
                      <button
                        type="button"
                        onClick={toggleExpanded}
                        className="text-left text-slate-200 underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                      >
                        {emitter.name}
                      </button>
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
                          <option value={NEW_ENTITY_VALUE}>
                            + New entity…
                          </option>
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
                  {isExpanded && (
                    <EmitterDetail
                      emitter={emitter}
                      rowBusy={rowBusy}
                      onToggleMatchEnabled={() =>
                        handleToggleMatchEnabled(emitter)
                      }
                      onToggleRandomized={() => handleToggleRandomized(emitter)}
                    />
                  )}
                </Fragment>
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

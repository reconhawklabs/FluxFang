// Task 9.5: browse discovered emitters, inspect each one's match rule and
// recent emissions, and manage its entity grouping.
//
// "Create entity & associate" here is deliberately NOT
// `POST /api/emitters/with-entity` — that endpoint (Task 6.4,
// `fluxfang-api::emitters`'s `create_with_entity`) creates a *brand-new*
// emitter row atomically with the entity, which is exactly what Task 9.4's
// Emissions page wants ("assign this batch of emissions to a fresh
// emitter") but wrong here: every row on this page is an *already-existing*
// emitter (from `GET /api/emitters`) whose id, `match_criteria`, and
// emission history must survive the association unchanged. So "create
// entity & associate" instead does the two-call sequence the task brief's
// context section spells out for this exact case: `POST /api/entities
// {name}` followed by `PATCH /api/emitters/:id {entity_id}` — see
// `createAndAssociateMutation` below.
import { Fragment, useMemo, useState } from 'react';
import type { ChangeEvent, FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { createEntity, listEntities } from '../api/entities';
import type { Entity } from '../api/entities';
import { deleteEmitter, listEmitters, patchEmitter } from '../api/emitters';
import type { Emitter } from '../api/emitters';
import { listEmissions } from '../api/emissions';
import type { Emission } from '../api/emissions';
import type { Condition, Rule } from '../types/rule';
import EmissionsHeatmap from '../components/EmissionsHeatmap';
import type { HeatmapPoint } from '../components/mapData';

const DETAIL_EMISSIONS_LIMIT = 20;
// Task C (emitter auto-classification design doc): a separate, larger-limit
// fetch backs the detection heatmap — the "Recent emissions" table above
// only needs its own last-20 window, but "everywhere this emitter has been
// heard" wants a much wider sample, same 500 cap `MapView.tsx`'s overview
// heatmap uses.
const HEATMAP_EMISSIONS_LIMIT = 500;
// Name, Type, Attributes, First Seen, Last Seen, Entity, Rule, Actions.
const TABLE_COLUMN_COUNT = 8;

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const selectClassName =
  'rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 focus:border-amber-500 focus:outline-none';
const smallButtonClassName =
  'rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50';

function formatTimestamp(iso: string | null): string {
  if (!iso) return '—';
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** `match_criteria` comes back from the backend as untyped
 * `serde_json::Value` (see `Emitter.match_criteria`'s doc comment) — an
 * emitter created with no rule at all persists `{}`, which doesn't satisfy
 * the `Rule` shape (`conditions` absent). This narrows defensively rather
 * than assuming every row has a well-formed `Rule`. */
function asRule(matchCriteria: unknown): Rule | null {
  if (!matchCriteria || typeof matchCriteria !== 'object') return null;
  const conditions = (matchCriteria as { conditions?: unknown }).conditions;
  if (!Array.isArray(conditions)) return null;
  return matchCriteria as Rule;
}

function formatConditionValue(value: unknown): string {
  if (Array.isArray(value)) return value.map((entry) => String(entry)).join(', ');
  return String(value);
}

function formatCondition(condition: Condition): string {
  return `${condition.field} ${condition.op} ${formatConditionValue(condition.value)}`;
}

/** Compact one-line rule summary for the table column, e.g. "bssid eq
 * aa:bb:cc:dd:ee:ff" or "bssid eq aa:.. AND ssid eq CoffeeShop" for
 * multiple conditions. Rendered monospace by the caller since these are
 * mostly MAC-ish identifying values, not prose. */
function summarizeRule(matchCriteria: unknown): string {
  const rule = asRule(matchCriteria);
  if (!rule || rule.conditions.length === 0) return '—';
  const joiner = rule.match === 'any' ? ' OR ' : ' AND ';
  return rule.conditions.map(formatCondition).join(joiner);
}

/** Full, readable rule description for the expanded detail panel — same
 * per-condition text as `summarizeRule` but with the match mode spelled out
 * and one condition per line (via the caller's `<ul>`) rather than packed
 * onto one line. */
function ruleConditions(matchCriteria: unknown): string[] {
  const rule = asRule(matchCriteria);
  if (!rule) return [];
  return rule.conditions.map(formatCondition);
}

function ruleMatchModeLabel(matchCriteria: unknown): string {
  const rule = asRule(matchCriteria);
  return rule?.match === 'any' ? 'ANY' : 'ALL';
}

function payloadText(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === 'string' || typeof value === 'number' ? String(value) : '—';
}

/** Reads a string attribute out of an emitter's `attributes` bag (Phase A
 * backend's `emitter.attributes jsonb`) — defensive same as `payloadText`,
 * since `attributes`' shape depends on `emitter_type` and older/plain
 * emitters carry `{}`. */
function attributeText(attributes: Record<string, unknown>, key: string): string | null {
  const value = attributes[key];
  return typeof value === 'string' ? value : null;
}

function isRandomizedMac(attributes: Record<string, unknown>): boolean {
  return attributes.randomized_mac === true;
}

/** The "type badge" (design doc's Frontend section): `type_label` when the
 * emitter is auto-classified (e.g. "WiFi Access Point"), falling back to the
 * free-text `type` a manually-created emitter carries, and finally "—" for
 * neither. */
function TypeBadge({ emitter }: { emitter: Emitter }) {
  const label = emitter.type_label ?? emitter.type;
  if (!label) return <span className="text-slate-500">—</span>;
  return (
    <span
      data-testid={`emitter-type-badge-${emitter.id}`}
      className="inline-block rounded bg-slate-800 px-2 py-0.5 text-xs font-medium text-slate-200"
    >
      {label}
    </span>
  );
}

/** Key identifying attributes shown per the domain rules (design doc): an
 * AP shows its SSID (or "Hidden" for an empty one) plus BSSID; a client
 * shows its source MAC plus a "Randomized MAC" badge when flagged. MAC/BSSID
 * are rendered monospace, same convention as the emissions table's bssid
 * column. Anything else (a plain/unclassified emitter) renders "—". */
function AttributesSummary({ emitter }: { emitter: Emitter }) {
  const attributes = emitter.attributes ?? {};

  if (emitter.emitter_type === 'wifi_access_point') {
    const ssid = attributeText(attributes, 'ssid');
    const bssid = attributeText(attributes, 'bssid');
    return (
      <div className="space-y-0.5">
        <div className="text-slate-300">{ssid && ssid.length > 0 ? ssid : 'Hidden'}</div>
        {bssid && <div className="font-mono text-xs text-slate-400">{bssid}</div>}
      </div>
    );
  }

  if (emitter.emitter_type === 'wifi_client') {
    const srcMac = attributeText(attributes, 'src_mac');
    return (
      <div className="space-y-1">
        {srcMac && <div className="font-mono text-xs text-slate-300">{srcMac}</div>}
        {isRandomizedMac(attributes) && (
          <span className="inline-block rounded bg-amber-500/20 px-1.5 py-0.5 text-[10px] font-medium text-amber-400">
            Randomized MAC
          </span>
        )}
      </div>
    );
  }

  return <span className="text-slate-500">—</span>;
}

interface EmitterDetailProps {
  emitter: Emitter;
}

/** The expanded row's content: the emitter's full match rule plus its most
 * recent emissions (`GET /api/emissions?emitter_id=:id&limit=20`). Its own
 * component (rather than inline in the parent) so the recent-emissions
 * query only ever runs while the row is actually expanded — it unmounts
 * (and the query goes away) when the row collapses. */
function EmitterDetail({ emitter }: EmitterDetailProps) {
  const params = useMemo(() => {
    const p = new URLSearchParams();
    p.set('emitter_id', emitter.id);
    p.set('limit', String(DETAIL_EMISSIONS_LIMIT));
    return p;
  }, [emitter.id]);

  // Keyed off `queryKeys.emissions` (per the registry) so a live `emission`
  // WS frame invalidating that key refetches this too, same convention as
  // `Emissions.tsx`'s own filtered query.
  const recentQuery = useQuery({
    queryKey: [...queryKeys.emissions, params.toString()],
    queryFn: () => listEmissions(params),
  });

  // Detection heatmap: "where this emitter has been heard" (design doc's
  // Frontend > Map section) — a wider, separately-fetched sample than the
  // "Recent emissions" table above, filtered client-side to only located
  // rows (an emission with no GPS fix has null `lon`/`lat`).
  const heatmapParams = useMemo(() => {
    const p = new URLSearchParams();
    p.set('emitter_id', emitter.id);
    p.set('limit', String(HEATMAP_EMISSIONS_LIMIT));
    return p;
  }, [emitter.id]);

  const heatmapQuery = useQuery({
    queryKey: [...queryKeys.emissions, 'heatmap', emitter.id],
    queryFn: () => listEmissions(heatmapParams),
  });

  const heatmapPoints = useMemo<HeatmapPoint[]>(() => {
    const items = heatmapQuery.data?.items ?? [];
    return items
      .filter((item): item is Emission & { lon: number; lat: number } => item.lon !== null && item.lat !== null)
      .map((item) => ({ lon: item.lon, lat: item.lat }));
  }, [heatmapQuery.data]);

  const conditions = ruleConditions(emitter.match_criteria);
  const items = recentQuery.data?.items ?? [];

  return (
    <tr data-testid={`emitter-detail-${emitter.id}`} className="border-b border-slate-900 bg-slate-950/40">
      <td colSpan={TABLE_COLUMN_COUNT} className="px-4 py-3">
        <div className="space-y-4">
          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Match rule</h3>
            {conditions.length === 0 ? (
              <p className="mt-1 text-sm text-slate-500">
                No conditions — this emitter doesn&apos;t auto-attach new emissions.
              </p>
            ) : (
              <div className="mt-1 text-sm text-slate-300">
                <span className="text-slate-500">Match {ruleMatchModeLabel(emitter.match_criteria)} of:</span>
                <ul className="mt-1 list-inside list-disc space-y-0.5 font-mono text-slate-200">
                  {conditions.map((text, index) => (
                    <li key={index}>{text}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>

          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Detection heatmap</h3>
            <p className="mt-1 text-xs text-slate-500">Where this emitter has been heard.</p>
            <div className="mt-1">
              <EmissionsHeatmap points={heatmapPoints} />
            </div>
          </div>

          <div>
            <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Recent emissions</h3>
            {recentQuery.isLoading && <p className="mt-1 text-sm text-slate-500">Loading emissions…</p>}
            {recentQuery.isError && (
              <p className="mt-1 text-sm text-red-400">Failed to load recent emissions.</p>
            )}
            {recentQuery.data && items.length === 0 && (
              <p className="mt-1 text-sm text-slate-500">No emissions recorded for this emitter yet.</p>
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
                    <tr key={emission.id} data-testid={`emitter-detail-emission-${emission.id}`}>
                      <td className="py-1 pr-4 text-slate-300">{formatTimestamp(emission.observed_at)}</td>
                      <td className="py-1 pr-4 font-mono text-slate-300">
                        {payloadText(emission.payload, 'bssid')}
                      </td>
                      <td className="py-1 pr-4 text-slate-300">{payloadText(emission.payload, 'ssid')}</td>
                      <td className="py-1 pr-4 font-mono text-slate-300">{emission.signal_strength ?? '—'}</td>
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

interface NewEntityFormProps {
  emitter: Emitter;
  submitting: boolean;
  errorMessage: string | null;
  onCancel: () => void;
  onSubmit: (name: string) => void;
}

/** Inline "create entity & associate" form — a name field plus
 * submit/cancel, shown per-row when its "New entity" toggle is open. */
function NewEntityForm({ emitter, submitting, errorMessage, onCancel, onSubmit }: NewEntityFormProps) {
  const [name, setName] = useState('');
  const fieldId = `new-entity-name-${emitter.id}`;

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    onSubmit(trimmed);
  }

  return (
    <form onSubmit={handleSubmit} className="mt-2 flex items-center gap-2">
      <label htmlFor={fieldId} className="sr-only">
        New entity name for {emitter.name}
      </label>
      <input
        id={fieldId}
        type="text"
        required
        autoFocus
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder="Entity name"
        className={`${inputClassName} max-w-[10rem]`}
      />
      <button
        type="submit"
        disabled={submitting || name.trim().length === 0}
        className="rounded bg-amber-500 px-2 py-1 text-xs font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {submitting ? 'Creating…' : 'Create & associate'}
      </button>
      <button type="button" onClick={onCancel} className={smallButtonClassName}>
        Cancel
      </button>
      {errorMessage && (
        <p role="alert" className="text-xs text-red-400">
          {errorMessage}
        </p>
      )}
    </form>
  );
}

export default function Emitters() {
  const queryClient = useQueryClient();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [creatingEntityForId, setCreatingEntityForId] = useState<string | null>(null);
  const [editingNameForId, setEditingNameForId] = useState<string | null>(null);
  const [editingNameDraft, setEditingNameDraft] = useState('');

  // Interim `{limit: 500}` cap on both lists — `GET /api/emitters` and
  // `GET /api/entities` now return a paginated `{items, total}` envelope,
  // but this page still renders every row with no pagination UI of its own
  // (that's a later redesign phase); 500 keeps today's "show everything"
  // behavior intact without adding pagination controls here.
  const emittersQuery = useQuery({ queryKey: queryKeys.emitters, queryFn: () => listEmitters({ limit: 500 }) });
  const entitiesQuery = useQuery({ queryKey: queryKeys.entities, queryFn: () => listEntities({ limit: 500 }) });

  const entityNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const entity of entitiesQuery.data?.items ?? []) map.set(entity.id, entity.name);
    return map;
  }, [entitiesQuery.data]);

  function invalidateEmitters(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
  }

  // Associate-to-existing (the row's entity <select>) and detach (the
  // "Detach" button, `{ entity_id: null }`) both go through this one
  // mutation — they're the same `PATCH /api/emitters/:id` call with a
  // different `entity_id`. Also doubles as the rename mutation (`{ name }`).
  const patchMutation = useMutation({
    mutationFn: ({ id, body }: { id: string; body: Parameters<typeof patchEmitter>[1] }) => patchEmitter(id, body),
    onSuccess: invalidateEmitters,
  });

  // "Create entity & associate": two sequential calls against an
  // *existing* emitter — `POST /api/entities` then `PATCH
  // /api/emitters/:id { entity_id }` — see the module doc comment for why
  // this isn't `POST /api/emitters/with-entity`. Invalidates both
  // `queryKeys.entities` (the new entity needs to show up in every row's
  // dropdown) and `queryKeys.emitters` (this row's own entity column).
  const createAndAssociateMutation = useMutation({
    mutationFn: async ({ emitterId, entityName }: { emitterId: string; entityName: string }) => {
      const entity = await createEntity({ name: entityName });
      const emitter = await patchEmitter(emitterId, { entity_id: entity.id });
      return { entity, emitter };
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
      invalidateEmitters();
      setCreatingEntityForId(null);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: deleteEmitter,
    onSuccess: invalidateEmitters,
  });

  function handleAssociateChange(emitterId: string, event: ChangeEvent<HTMLSelectElement>): void {
    const entityId = event.target.value;
    if (entityId.length === 0) return;
    patchMutation.mutate({ id: emitterId, body: { entity_id: entityId } });
  }

  function handleDetach(emitterId: string): void {
    patchMutation.mutate({ id: emitterId, body: { entity_id: null } });
  }

  // The rule enable/disable toggle (design doc's "Rules are visible +
  // toggleable" section) — a plain `{ match_enabled }` PATCH, flipping
  // whatever the emitter's current value is.
  function handleToggleMatchEnabled(emitter: Emitter): void {
    patchMutation.mutate({ id: emitter.id, body: { match_enabled: !emitter.match_enabled } });
  }

  // Manual randomized-MAC override (design doc's Frontend section) — since
  // `PATCH .../attributes` is a full replace, not a merge (see
  // `PatchEmitterInput`'s doc comment in `api/emitters.ts`), this reads the
  // emitter's *already-loaded* `attributes` (no extra GET needed — it's the
  // same object this row is rendering from `queryKeys.emitters`), spreads
  // it, and flips just `randomized_mac` before sending the whole thing.
  function handleToggleRandomized(emitter: Emitter): void {
    const currentAttributes = emitter.attributes ?? {};
    const currentlyRandomized = isRandomizedMac(currentAttributes);
    patchMutation.mutate({
      id: emitter.id,
      body: { attributes: { ...currentAttributes, randomized_mac: !currentlyRandomized } },
    });
  }

  function handleCreateAndAssociate(emitterId: string, entityName: string): void {
    createAndAssociateMutation.mutate({ emitterId, entityName });
  }

  function startEditingName(emitter: Emitter): void {
    setEditingNameForId(emitter.id);
    setEditingNameDraft(emitter.name);
  }

  function commitEditingName(emitterId: string): void {
    const trimmed = editingNameDraft.trim();
    if (trimmed.length > 0) {
      patchMutation.mutate({ id: emitterId, body: { name: trimmed } });
    }
    setEditingNameForId(null);
  }

  function handleDelete(emitterId: string): void {
    if (!window.confirm('Delete this emitter?')) return;
    deleteMutation.mutate(emitterId);
  }

  const createAndAssociateErrorMessage =
    createAndAssociateMutation.error instanceof ApiError
      ? createAndAssociateMutation.error.message
      : createAndAssociateMutation.isError
        ? 'Failed to create entity.'
        : null;

  const emitters = emittersQuery.data?.items ?? [];

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold text-slate-100">Emitters</h1>

      {emittersQuery.isLoading && <p className="text-sm text-slate-500">Loading emitters…</p>}
      {emittersQuery.isError && <p className="text-sm text-red-400">Failed to load emitters.</p>}
      {emittersQuery.data && emitters.length === 0 && (
        <p className="text-sm text-slate-500">No emitters discovered yet.</p>
      )}

      {emitters.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Type</th>
              <th className="py-2 pr-4 font-medium">Attributes</th>
              <th className="py-2 pr-4 font-medium">First Seen</th>
              <th className="py-2 pr-4 font-medium">Last Seen</th>
              <th className="py-2 pr-4 font-medium">Entity</th>
              <th className="py-2 pr-4 font-medium">Rule</th>
              <th className="py-2 pr-4 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {emitters.map((emitter) => {
              const isExpanded = expandedId === emitter.id;
              const isEditingName = editingNameForId === emitter.id;
              const isCreatingEntity = creatingEntityForId === emitter.id;
              const entityName = emitter.entity_id ? (entityNameById.get(emitter.entity_id) ?? '—') : '—';
              const rowBusy =
                (patchMutation.isPending && patchMutation.variables?.id === emitter.id) ||
                (deleteMutation.isPending && deleteMutation.variables === emitter.id) ||
                (createAndAssociateMutation.isPending &&
                  createAndAssociateMutation.variables?.emitterId === emitter.id);

              return (
                <Fragment key={emitter.id}>
                  <tr data-testid={`emitter-row-${emitter.id}`} className="border-b border-slate-900 align-top">
                    <td className="py-2 pr-4 text-slate-200">
                      <button
                        type="button"
                        onClick={() => setExpandedId(isExpanded ? null : emitter.id)}
                        className="text-left text-slate-200 underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                      >
                        {emitter.name}
                      </button>
                      {isEditingName ? (
                        <div className="mt-1 flex items-center gap-1">
                          <label htmlFor={`emitter-name-${emitter.id}`} className="sr-only">
                            Edit name for {emitter.name}
                          </label>
                          <input
                            id={`emitter-name-${emitter.id}`}
                            type="text"
                            value={editingNameDraft}
                            onChange={(event) => setEditingNameDraft(event.target.value)}
                            className={`${inputClassName} max-w-[9rem] py-1 text-xs`}
                          />
                          <button
                            type="button"
                            onClick={() => commitEditingName(emitter.id)}
                            className={smallButtonClassName}
                          >
                            Save
                          </button>
                          <button
                            type="button"
                            onClick={() => setEditingNameForId(null)}
                            className={smallButtonClassName}
                          >
                            Cancel
                          </button>
                        </div>
                      ) : (
                        <button
                          type="button"
                          onClick={() => startEditingName(emitter)}
                          className="mt-1 block text-xs text-slate-500 underline decoration-dotted hover:text-amber-400"
                        >
                          Rename
                        </button>
                      )}
                    </td>
                    <td className="py-2 pr-4">
                      <TypeBadge emitter={emitter} />
                    </td>
                    <td className="py-2 pr-4">
                      <AttributesSummary emitter={emitter} />
                      {emitter.emitter_type === 'wifi_client' && (
                        <button
                          type="button"
                          disabled={rowBusy}
                          onClick={() => handleToggleRandomized(emitter)}
                          className="mt-1 block text-[10px] text-slate-500 underline decoration-dotted hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {isRandomizedMac(emitter.attributes ?? {})
                            ? 'Mark as not randomized'
                            : 'Mark as randomized'}
                        </button>
                      )}
                    </td>
                    <td className="py-2 pr-4 text-slate-300">{formatTimestamp(emitter.first_seen_at)}</td>
                    <td className="py-2 pr-4 text-slate-300">{formatTimestamp(emitter.last_seen_at)}</td>
                    <td data-testid={`emitter-entity-${emitter.id}`} className="py-2 pr-4 text-slate-300">
                      {entityName}
                    </td>
                    <td className="py-2 pr-4">
                      <div className="font-mono text-xs text-slate-300">
                        {summarizeRule(emitter.match_criteria)}
                      </div>
                      <label className="mt-1 flex items-center gap-1.5 text-xs text-slate-400">
                        <input
                          type="checkbox"
                          role="switch"
                          aria-label={`Rule enabled for ${emitter.name}`}
                          checked={emitter.match_enabled}
                          disabled={rowBusy}
                          onChange={() => handleToggleMatchEnabled(emitter)}
                          className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                        />
                        {emitter.match_enabled ? 'Enabled' : 'Disabled'}
                      </label>
                      {!emitter.match_enabled && (
                        <p className="mt-1 max-w-[10rem] text-[10px] text-amber-400">
                          Disabled — new matching emissions won&apos;t auto-attach.
                        </p>
                      )}
                    </td>
                    <td className="py-2 pr-4">
                      <div className="flex flex-wrap items-center gap-2">
                        <select
                          aria-label={`Associate ${emitter.name} to an entity`}
                          value=""
                          disabled={rowBusy}
                          onChange={(event) => handleAssociateChange(emitter.id, event)}
                          className={selectClassName}
                        >
                          <option value="" disabled>
                            Associate to entity…
                          </option>
                          {(entitiesQuery.data?.items ?? []).map((entity: Entity) => (
                            <option key={entity.id} value={entity.id}>
                              {entity.name}
                            </option>
                          ))}
                        </select>

                        {emitter.entity_id && (
                          <button
                            type="button"
                            disabled={rowBusy}
                            onClick={() => handleDetach(emitter.id)}
                            className={smallButtonClassName}
                          >
                            Detach
                          </button>
                        )}

                        {!isCreatingEntity && (
                          <button
                            type="button"
                            disabled={rowBusy}
                            onClick={() => setCreatingEntityForId(emitter.id)}
                            className={smallButtonClassName}
                          >
                            New entity
                          </button>
                        )}

                        <button
                          type="button"
                          disabled={rowBusy}
                          onClick={() => handleDelete(emitter.id)}
                          className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {deleteMutation.isPending && deleteMutation.variables === emitter.id
                            ? 'Deleting…'
                            : 'Delete'}
                        </button>
                      </div>

                      {isCreatingEntity && (
                        <NewEntityForm
                          emitter={emitter}
                          submitting={
                            createAndAssociateMutation.isPending &&
                            createAndAssociateMutation.variables?.emitterId === emitter.id
                          }
                          errorMessage={
                            createAndAssociateMutation.variables?.emitterId === emitter.id
                              ? createAndAssociateErrorMessage
                              : null
                          }
                          onCancel={() => setCreatingEntityForId(null)}
                          onSubmit={(name) => handleCreateAndAssociate(emitter.id, name)}
                        />
                      )}
                    </td>
                  </tr>
                  {isExpanded && <EmitterDetail emitter={emitter} />}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

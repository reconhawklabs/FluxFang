// Task 9.6: manage tracked entities (a real-world subject an operator has
// grouped one or more emitters under, e.g. "Bob's phone"), inspect each
// one's associated emitters + aggregate last-seen/live status, and build
// per-entity alert rules.
//
// "Add Alert" builds a `POST /api/alert-rules` body scoped to this entity
// (`target_type: "entity", target_id: <entity.id>`) — trigger type dropdown
// (detected / enters zone / leaves zone, per
// `fluxfang-api::alert_rules`'s validation matrix), a zone picker
// (`GET /api/zones`) that only appears for a zone trigger, an optional
// content-match `RuleBuilder` (Task 9.2, kind="wifi"), and a method
// multi-select sourced from `GET /api/alert-methods` (Task 6.6 — this page
// only *consumes* that list; managing Alert Methods themselves is Task
// 9.9's Alerts page). The form itself is rendered as a page-level fixed
// overlay (state lives in the top-level `Entities` component, same as
// `AddEntityForm`), not nested inside the expanded row's `<tr>` — a `<tr>`
// can only validly contain table cells, and React's direct-DOM-API
// insertion doesn't foster-parent stray children out the way the HTML
// parser would.
import { Fragment, useState } from 'react';
import type { ChangeEvent, FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { createEntity, deleteEntity, getEntityDetail, listEntities, patchEntity } from '../api/entities';
import type { CreateEntityInput, Entity, PatchEntityInput } from '../api/entities';
import { listZones } from '../api/zones';
import { listAlertMethods } from '../api/alertMethods';
import { createAlertRule, listAlertRules } from '../api/alertRules';
import type { AlertRuleTrigger, AlertRuleTriggerOn, CreateAlertRuleInput } from '../api/alertRules';
import RuleBuilder from '../components/RuleBuilder';
import type { Rule } from '../types/rule';

const TABLE_COLUMN_COUNT = 3;

/** How recently an entity must have been seen (its aggregate `last_seen`)
 * to show as "live" (green dot) rather than "stale" (gray dot). Purely a
 * UI freshness cue — the backend doesn't define a "live" concept itself. */
const LIVE_WINDOW_MS = 5 * 60 * 1000;

const EMPTY_RULE: Rule = { match: 'all', conditions: [] };

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';
const smallButtonClassName =
  'rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50';
const cancelButtonClassName =
  'rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100';
const submitButtonClassName =
  'rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50';

function formatTimestamp(iso: string | null): string {
  if (!iso) return '—';
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function isRecentlySeen(iso: string | null): boolean {
  if (!iso) return false;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return false;
  return Date.now() - date.getTime() <= LIVE_WINDOW_MS;
}

/** The aggregate last-seen cell: a colored dot (green = seen within
 * `LIVE_WINDOW_MS`, gray = seen longer ago, dark = never) plus the
 * formatted timestamp ("Never" when `lastSeen` is null). */
function LastSeenStatus({ lastSeen }: { lastSeen: string | null }) {
  const recent = isRecentlySeen(lastSeen);
  const dotClassName = lastSeen === null ? 'bg-slate-700' : recent ? 'bg-green-500' : 'bg-slate-500';

  return (
    <span className="inline-flex items-center gap-1.5">
      <span data-testid="entity-live-indicator" className={`inline-block h-2 w-2 rounded-full ${dotClassName}`} />
      <span className="text-slate-300">{lastSeen === null ? 'Never' : formatTimestamp(lastSeen)}</span>
    </span>
  );
}

const TRIGGER_OPTIONS: { value: AlertRuleTriggerOn; label: string }[] = [
  { value: 'detected', label: 'When detected' },
  { value: 'enters_zone', label: 'When enters zone' },
  { value: 'leaves_zone', label: 'When leaves zone' },
];

interface AddAlertRuleFormProps {
  entity: Entity;
  onCancel: () => void;
  onCreated: () => void;
}

/** The per-entity "add alert" form: name, trigger-type dropdown (revealing
 * a zone picker only for a zone trigger), an optional content-match
 * `RuleBuilder`, and an alert-method multi-select. Submits `POST
 * /api/alert-rules` with `target_type: "entity"`/`target_id: entity.id`. */
function AddAlertRuleForm({ entity, onCancel, onCreated }: AddAlertRuleFormProps) {
  const [name, setName] = useState('');
  const [on, setOn] = useState<AlertRuleTriggerOn>('detected');
  const [zoneId, setZoneId] = useState('');
  const [matchContent, setMatchContent] = useState(false);
  const [contentMatch, setContentMatch] = useState<Rule>(EMPTY_RULE);
  const [methodIds, setMethodIds] = useState<string[]>([]);

  const isZoneTrigger = on === 'enters_zone' || on === 'leaves_zone';

  const zonesQuery = useQuery({ queryKey: queryKeys.zones, queryFn: listZones });
  const methodsQuery = useQuery({ queryKey: queryKeys.alertMethods, queryFn: listAlertMethods });

  const createMutation = useMutation({
    mutationFn: (input: CreateAlertRuleInput) => createAlertRule(input),
    onSuccess: onCreated,
  });

  function handleTriggerChange(event: ChangeEvent<HTMLSelectElement>): void {
    const next = event.target.value as AlertRuleTriggerOn;
    setOn(next);
    if (next === 'detected') setZoneId('');
  }

  function toggleMethod(id: string, checked: boolean): void {
    setMethodIds((prev) => (checked ? [...prev, id] : prev.filter((existing) => existing !== id)));
  }

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = name.trim();
    if (trimmedName.length === 0) return;
    if (isZoneTrigger && zoneId.length === 0) return;

    const trigger: AlertRuleTrigger = { on };
    if (isZoneTrigger) trigger.zone_id = zoneId;
    if (matchContent) trigger.content_match = contentMatch;

    createMutation.mutate({
      name: trimmedName,
      enabled: true,
      target_type: 'entity',
      target_id: entity.id,
      trigger,
      method_ids: methodIds,
    });
  }

  const errorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create alert rule.'
        : null;

  const methods = methodsQuery.data ?? [];

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="max-h-[90vh] w-full max-w-lg space-y-4 overflow-y-auto rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">Add Alert for {entity.name}</h2>

        <div className="space-y-1">
          <label htmlFor="alert-rule-name" className={labelClassName}>
            Name
          </label>
          <input
            id="alert-rule-name"
            type="text"
            required
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="alert-rule-trigger" className={labelClassName}>
            Trigger
          </label>
          <select id="alert-rule-trigger" value={on} onChange={handleTriggerChange} className={inputClassName}>
            {TRIGGER_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>

        {isZoneTrigger && (
          <div className="space-y-1">
            <label htmlFor="alert-rule-zone" className={labelClassName}>
              Zone
            </label>
            <select
              id="alert-rule-zone"
              required
              value={zoneId}
              onChange={(event) => setZoneId(event.target.value)}
              className={inputClassName}
            >
              <option value="" disabled>
                Select a zone…
              </option>
              {(zonesQuery.data ?? []).map((zone) => (
                <option key={zone.id} value={zone.id}>
                  {zone.name}
                </option>
              ))}
            </select>
            {zonesQuery.isError && <p className="text-xs text-red-400">Failed to load zones.</p>}
          </div>
        )}

        <div className="space-y-2">
          <label className="flex items-center gap-2 text-sm text-slate-300">
            <input
              type="checkbox"
              checked={matchContent}
              onChange={(event) => setMatchContent(event.target.checked)}
            />
            Only when emission matches…
          </label>
          {matchContent && <RuleBuilder kind="wifi" value={contentMatch} onChange={setContentMatch} />}
        </div>

        <div className="space-y-1">
          <span className={labelClassName}>Alert methods</span>
          {methodsQuery.isLoading && <p className="text-sm text-slate-500">Loading alert methods…</p>}
          {methodsQuery.isError && <p className="text-sm text-red-400">Failed to load alert methods.</p>}
          {methodsQuery.data && methods.length === 0 && (
            <p className="text-sm text-slate-500">
              No alert methods configured yet — add one on the Alerts page first.
            </p>
          )}
          {methods.length > 0 && (
            <div className="space-y-1">
              {methods.map((method) => (
                <label key={method.id} className="flex items-center gap-2 text-sm text-slate-300">
                  <input
                    type="checkbox"
                    checked={methodIds.includes(method.id)}
                    onChange={(event) => toggleMethod(method.id, event.target.checked)}
                  />
                  {method.name} <span className="text-slate-500">({method.type})</span>
                </label>
              ))}
            </div>
          )}
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
          <button type="submit" disabled={createMutation.isPending} className={submitButtonClassName}>
            {createMutation.isPending ? 'Creating…' : 'Create Alert Rule'}
          </button>
        </div>
      </form>
    </div>
  );
}

interface EntityDetailProps {
  entity: Entity;
  onDeleted: () => void;
  onAddAlert: () => void;
}

/** The expanded row's content: fetches `GET /api/entities/:id` for the
 * associated emitters + aggregate `last_seen`/`recent_detections`, lets the
 * name/notes be edited or the entity deleted, and lists the entity's
 * existing alert rules (client-filtered from `GET /api/alert-rules` by
 * `target_id`). Its own component (rather than inline in the parent) so
 * the detail/alert-rules queries only run while the row is actually
 * expanded, same convention as `Emitters.tsx`'s `EmitterDetail`. */
function EntityDetail({ entity, onDeleted, onAddAlert }: EntityDetailProps) {
  const queryClient = useQueryClient();
  const [isEditing, setIsEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState(entity.name);
  const [notesDraft, setNotesDraft] = useState(entity.notes ?? '');

  const detailQuery = useQuery({
    queryKey: [...queryKeys.entities, entity.id],
    queryFn: () => getEntityDetail(entity.id),
  });

  const alertRulesQuery = useQuery({ queryKey: queryKeys.alertRules, queryFn: listAlertRules });

  function invalidateEntities(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
  }

  const patchMutation = useMutation({
    mutationFn: (body: PatchEntityInput) => patchEntity(entity.id, body),
    onSuccess: () => {
      invalidateEntities();
      setIsEditing(false);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteEntity(entity.id),
    onSuccess: () => {
      invalidateEntities();
      onDeleted();
    },
  });

  function handleSaveEdit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = nameDraft.trim();
    if (trimmedName.length === 0) return;
    const trimmedNotes = notesDraft.trim();
    patchMutation.mutate({ name: trimmedName, notes: trimmedNotes.length > 0 ? trimmedNotes : null });
  }

  function handleDelete(): void {
    if (!window.confirm(`Delete ${entity.name}? Its emitters will be detached, not deleted.`)) return;
    deleteMutation.mutate();
  }

  const patchErrorMessage =
    patchMutation.error instanceof ApiError
      ? patchMutation.error.message
      : patchMutation.isError
        ? 'Failed to save changes.'
        : null;

  const detail = detailQuery.data;
  const rulesForEntity = (alertRulesQuery.data ?? []).filter((rule) => rule.target_id === entity.id);

  return (
    <tr data-testid={`entity-detail-${entity.id}`} className="border-b border-slate-900 bg-slate-950/40">
      <td colSpan={TABLE_COLUMN_COUNT} className="px-4 py-3">
        <div className="space-y-4">
          {detailQuery.isLoading && <p className="text-sm text-slate-500">Loading entity…</p>}
          {detailQuery.isError && <p className="text-sm text-red-400">Failed to load entity detail.</p>}

          {detail && (
            <>
              <div>
                <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Last seen</h3>
                <div data-testid={`entity-last-seen-${entity.id}`} className="mt-1 text-sm">
                  <LastSeenStatus lastSeen={detail.last_seen} />
                </div>
                <p className="mt-1 text-xs text-slate-500">
                  {detail.recent_detections.length} recent detection{detail.recent_detections.length === 1 ? '' : 's'}
                </p>
              </div>

              <div>
                <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Emitters</h3>
                {detail.emitters.length === 0 ? (
                  <p className="mt-1 text-sm text-slate-500">No emitters associated with this entity yet.</p>
                ) : (
                  <table className="mt-1 w-full border-collapse text-left text-xs">
                    <thead>
                      <tr className="border-b border-slate-800 text-slate-500">
                        <th className="py-1 pr-4 font-medium">Name</th>
                        <th className="py-1 pr-4 font-medium">Type</th>
                        <th className="py-1 pr-4 font-medium">Last Seen</th>
                      </tr>
                    </thead>
                    <tbody>
                      {detail.emitters.map((emitter) => (
                        <tr key={emitter.id} data-testid={`entity-detail-emitter-${emitter.id}`}>
                          <td className="py-1 pr-4 text-slate-200">{emitter.name}</td>
                          <td className="py-1 pr-4 text-slate-300">{emitter.type ?? '—'}</td>
                          <td className="py-1 pr-4 text-slate-300">{formatTimestamp(emitter.last_seen_at)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>

              <div>
                <div className="flex items-center justify-between">
                  <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Name / Notes</h3>
                  {!isEditing && (
                    <button type="button" onClick={() => setIsEditing(true)} className={smallButtonClassName}>
                      Edit
                    </button>
                  )}
                </div>
                {isEditing ? (
                  <form onSubmit={handleSaveEdit} className="mt-1 space-y-2">
                    <label htmlFor={`entity-edit-name-${entity.id}`} className="sr-only">
                      Edit name for {entity.name}
                    </label>
                    <input
                      id={`entity-edit-name-${entity.id}`}
                      type="text"
                      value={nameDraft}
                      onChange={(event) => setNameDraft(event.target.value)}
                      className={inputClassName}
                    />
                    <label htmlFor={`entity-edit-notes-${entity.id}`} className="sr-only">
                      Edit notes for {entity.name}
                    </label>
                    <textarea
                      id={`entity-edit-notes-${entity.id}`}
                      value={notesDraft}
                      onChange={(event) => setNotesDraft(event.target.value)}
                      className={`${inputClassName} min-h-[4rem]`}
                    />
                    {patchErrorMessage && (
                      <p role="alert" className="text-xs text-red-400">
                        {patchErrorMessage}
                      </p>
                    )}
                    <div className="flex gap-2">
                      <button type="submit" disabled={patchMutation.isPending} className={smallButtonClassName}>
                        {patchMutation.isPending ? 'Saving…' : 'Save'}
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setIsEditing(false);
                          setNameDraft(entity.name);
                          setNotesDraft(entity.notes ?? '');
                        }}
                        className={smallButtonClassName}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                ) : (
                  <p className="mt-1 text-sm text-slate-300">{entity.notes || 'No notes.'}</p>
                )}
              </div>

              <div>
                <div className="flex items-center justify-between">
                  <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Alert rules</h3>
                  <button type="button" onClick={onAddAlert} className={smallButtonClassName}>
                    Add Alert
                  </button>
                </div>
                {rulesForEntity.length === 0 ? (
                  <p className="mt-1 text-sm text-slate-500">No alert rules configured for this entity yet.</p>
                ) : (
                  <ul className="mt-1 space-y-0.5 text-sm text-slate-300">
                    {rulesForEntity.map((rule) => (
                      <li key={rule.id} data-testid={`entity-alert-rule-${rule.id}`}>
                        {rule.name} <span className="text-slate-500">— {rule.trigger.on}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>

              <div className="flex justify-end">
                <button
                  type="button"
                  onClick={handleDelete}
                  disabled={deleteMutation.isPending}
                  className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {deleteMutation.isPending ? 'Deleting…' : 'Delete entity'}
                </button>
              </div>
            </>
          )}
        </div>
      </td>
    </tr>
  );
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
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [addAlertEntity, setAddAlertEntity] = useState<Entity | null>(null);

  const entitiesQuery = useQuery({ queryKey: queryKeys.entities, queryFn: listEntities });

  function invalidateEntities(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
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

  const entities = entitiesQuery.data ?? [];

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

      {entitiesQuery.isLoading && <p className="text-sm text-slate-500">Loading entities…</p>}
      {entitiesQuery.isError && <p className="text-sm text-red-400">Failed to load entities.</p>}
      {entitiesQuery.data && entities.length === 0 && (
        <p className="text-sm text-slate-500">No entities yet.</p>
      )}

      {entities.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Notes</th>
              <th className="py-2 pr-4 font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {entities.map((entity) => {
              const isExpanded = expandedId === entity.id;
              return (
                <Fragment key={entity.id}>
                  <tr data-testid={`entity-row-${entity.id}`} className="border-b border-slate-900 align-top">
                    <td className="py-2 pr-4 text-slate-200">
                      <button
                        type="button"
                        onClick={() => setExpandedId(isExpanded ? null : entity.id)}
                        className="text-left text-slate-200 underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                      >
                        {entity.name}
                      </button>
                    </td>
                    <td className="py-2 pr-4 text-slate-300">{entity.notes ?? '—'}</td>
                    <td className="py-2 pr-4 text-slate-300">{formatTimestamp(entity.created_at)}</td>
                  </tr>
                  {isExpanded && (
                    <EntityDetail
                      entity={entity}
                      onDeleted={() => setExpandedId(null)}
                      onAddAlert={() => setAddAlertEntity(entity)}
                    />
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      )}

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

      {addAlertEntity && (
        <AddAlertRuleForm
          entity={addAlertEntity}
          onCancel={() => setAddAlertEntity(null)}
          onCreated={() => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.alertRules });
            setAddAlertEntity(null);
          }}
        />
      )}
    </div>
  );
}

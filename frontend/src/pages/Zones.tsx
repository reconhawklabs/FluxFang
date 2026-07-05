// Task 9.8: manage geofence zones — list, add/edit (lat/lon + radius +
// notes), and inspect which emitters/entities are currently inside one
// (`GET /api/zones/:id`'s `emitters`/`entities`, per
// `ZoneRepo::subjects_in_zone`).
//
// List-row subject counts: the brief allows either fetching per-zone detail
// counts for every row or deferring to a "View" that loads the detail. This
// page takes the latter (simpler, no N+1 `GET /api/zones/:id` fan-out just
// to populate a column) — the list itself shows name/center/radius/notes,
// and the subject list only appears once a row is expanded, same
// expand-to-detail convention as `Entities.tsx`/`Emitters.tsx`.
//
// A mini-map pin-drop (the brief's "nice to have") is intentionally omitted
// — YAGNI beyond the required lat/lon number inputs, which are simpler to
// test and don't need jsdom/WebGL guarding like `MapView`'s canvas does.
import { Fragment, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { createZone, deleteZone, getZoneDetail, listZones, patchZone } from '../api/zones';
import type { CreateZoneInput, PatchZoneInput, Zone } from '../api/zones';

const TABLE_COLUMN_COUNT = 4;

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';
const smallButtonClassName =
  'rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50';
const cancelButtonClassName =
  'rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100';
const submitButtonClassName =
  'rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50';

/** Same lon/lat/radius ranges as the backend's `validate_zone` (see
 * `fluxfang-api::zones`'s module doc comment) — kept client-side so an
 * out-of-range value never leaves the browser as a request. Returns the
 * first violation's message, or `null` when the form is valid. */
function validateZoneForm(lat: number, lon: number, radiusM: number): string | null {
  if (Number.isNaN(lat) || lat < -90 || lat > 90) {
    return `Latitude must be between -90 and 90, got ${Number.isNaN(lat) ? 'nothing' : lat}.`;
  }
  if (Number.isNaN(lon) || lon < -180 || lon > 180) {
    return `Longitude must be between -180 and 180, got ${Number.isNaN(lon) ? 'nothing' : lon}.`;
  }
  if (Number.isNaN(radiusM) || radiusM <= 0) {
    return `Radius must be greater than 0, got ${Number.isNaN(radiusM) ? 'nothing' : radiusM}.`;
  }
  return null;
}

interface ZoneFormValues {
  name: string;
  lat: string;
  lon: string;
  radiusM: string;
  notes: string;
}

function initialFormValues(zone: Zone | null): ZoneFormValues {
  if (!zone) return { name: '', lat: '', lon: '', radiusM: '', notes: '' };
  return {
    name: zone.name,
    lat: String(zone.lat),
    lon: String(zone.lon),
    radiusM: String(zone.radius_m),
    notes: zone.notes ?? '',
  };
}

interface ZoneFormProps {
  /** `null` for "Add Zone", the zone being edited for "Edit Zone". */
  zone: Zone | null;
  onCancel: () => void;
  onSubmit: (input: CreateZoneInput) => void;
  submitting: boolean;
  submitErrorMessage: string | null;
}

/** Shared add/edit modal — both build the same
 * `{name, center:{lon,lat}, radius_m, notes}` shape (`CreateZoneInput`,
 * reused as the `PATCH` body's shape too since this page always sends the
 * full form rather than a partial patch). Client-side validation
 * (`validateZoneForm`) runs before either mutation fires, so an
 * out-of-range lat/lon/radius never reaches `fetch`. */
function ZoneForm({ zone, onCancel, onSubmit, submitting, submitErrorMessage }: ZoneFormProps) {
  const isEditing = zone !== null;
  const [values, setValues] = useState<ZoneFormValues>(() => initialFormValues(zone));
  const [validationError, setValidationError] = useState<string | null>(null);

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = values.name.trim();
    if (trimmedName.length === 0) return;

    const lat = Number(values.lat);
    const lon = Number(values.lon);
    const radiusM = Number(values.radiusM);

    const error = validateZoneForm(lat, lon, radiusM);
    setValidationError(error);
    if (error) return;

    const trimmedNotes = values.notes.trim();
    onSubmit({
      name: trimmedName,
      center: { lon, lat },
      radius_m: radiusM,
      notes: trimmedNotes.length > 0 ? trimmedNotes : undefined,
    });
  }

  const errorMessage = validationError ?? submitErrorMessage;

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="max-h-[90vh] w-full max-w-md space-y-4 overflow-y-auto rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">{isEditing ? 'Edit Zone' : 'Add Zone'}</h2>

        <div className="space-y-1">
          <label htmlFor="zone-name" className={labelClassName}>
            Name
          </label>
          <input
            id="zone-name"
            type="text"
            required
            autoFocus
            value={values.name}
            onChange={(event) => setValues((prev) => ({ ...prev, name: event.target.value }))}
            className={inputClassName}
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1">
            <label htmlFor="zone-lat" className={labelClassName}>
              Latitude
            </label>
            <input
              id="zone-lat"
              type="number"
              step="any"
              required
              value={values.lat}
              onChange={(event) => setValues((prev) => ({ ...prev, lat: event.target.value }))}
              className={inputClassName}
            />
          </div>
          <div className="space-y-1">
            <label htmlFor="zone-lon" className={labelClassName}>
              Longitude
            </label>
            <input
              id="zone-lon"
              type="number"
              step="any"
              required
              value={values.lon}
              onChange={(event) => setValues((prev) => ({ ...prev, lon: event.target.value }))}
              className={inputClassName}
            />
          </div>
        </div>

        <div className="space-y-1">
          <label htmlFor="zone-radius" className={labelClassName}>
            Radius (meters)
          </label>
          <input
            id="zone-radius"
            type="number"
            step="any"
            required
            value={values.radiusM}
            onChange={(event) => setValues((prev) => ({ ...prev, radiusM: event.target.value }))}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="zone-notes" className={labelClassName}>
            Notes
          </label>
          <textarea
            id="zone-notes"
            value={values.notes}
            onChange={(event) => setValues((prev) => ({ ...prev, notes: event.target.value }))}
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
            {submitting ? (isEditing ? 'Saving…' : 'Adding…') : isEditing ? 'Save' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
}

interface ZoneDetailProps {
  zone: Zone;
  onEdit: () => void;
  onDeleted: () => void;
}

/** The expanded row's content: `GET /api/zones/:id`'s current subjects
 * (emitters/entities), plus edit/delete actions. Its own component (rather
 * than inline in the parent) so the detail query only runs while the row is
 * actually expanded, same convention as `Entities.tsx`'s `EntityDetail`/
 * `Emitters.tsx`'s `EmitterDetail`. */
function ZoneDetail({ zone, onEdit, onDeleted }: ZoneDetailProps) {
  const queryClient = useQueryClient();

  const detailQuery = useQuery({
    queryKey: [...queryKeys.zones, zone.id],
    queryFn: () => getZoneDetail(zone.id),
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteZone(zone.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.zones });
      onDeleted();
    },
  });

  function handleDelete(): void {
    if (
      !window.confirm(
        `Delete zone "${zone.name}"? Any alert rules watching it will be disabled, not deleted.`,
      )
    )
      return;
    deleteMutation.mutate();
  }

  const detail = detailQuery.data;
  const hasSubjects = detail && (detail.emitters.length > 0 || detail.entities.length > 0);

  return (
    <tr data-testid={`zone-detail-${zone.id}`} className="border-b border-slate-900 bg-slate-950/40">
      <td colSpan={TABLE_COLUMN_COUNT} className="px-4 py-3">
        <div className="space-y-4">
          {detailQuery.isLoading && <p className="text-sm text-slate-500">Loading zone…</p>}
          {detailQuery.isError && <p className="text-sm text-red-400">Failed to load zone detail.</p>}

          {detail && (
            <div>
              <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">Subjects in this zone</h3>
              {!hasSubjects ? (
                <p className="mt-1 text-sm text-slate-500">No subjects currently in this zone.</p>
              ) : (
                <div className="mt-1 space-y-3">
                  {detail.entities.length > 0 && (
                    <ul className="space-y-0.5 text-sm text-slate-300">
                      {detail.entities.map((entity) => (
                        <li key={entity.id} data-testid={`zone-detail-entity-${entity.id}`}>
                          {entity.name} <span className="text-slate-500">(entity)</span>
                        </li>
                      ))}
                    </ul>
                  )}
                  {detail.emitters.length > 0 && (
                    <ul className="space-y-0.5 text-sm text-slate-300">
                      {detail.emitters.map((emitter) => (
                        <li key={emitter.id} data-testid={`zone-detail-emitter-${emitter.id}`}>
                          {emitter.name} <span className="text-slate-500">({emitter.type ?? 'emitter'})</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              )}
            </div>
          )}

          <div className="flex justify-end gap-2">
            <button type="button" onClick={onEdit} className={smallButtonClassName}>
              Edit
            </button>
            <button
              type="button"
              onClick={handleDelete}
              disabled={deleteMutation.isPending}
              className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {deleteMutation.isPending ? 'Deleting…' : 'Delete zone'}
            </button>
          </div>
        </div>
      </td>
    </tr>
  );
}

export default function Zones() {
  const queryClient = useQueryClient();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [editingZone, setEditingZone] = useState<Zone | null>(null);

  const zonesQuery = useQuery({ queryKey: queryKeys.zones, queryFn: listZones });

  function invalidateZones(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.zones });
  }

  const createMutation = useMutation({
    mutationFn: (input: CreateZoneInput) => createZone(input),
    onSuccess: () => {
      invalidateZones();
      setShowAddForm(false);
    },
  });

  const patchMutation = useMutation({
    mutationFn: (input: CreateZoneInput) => {
      if (!editingZone) throw new Error('no zone being edited');
      return patchZone(editingZone.id, input as PatchZoneInput);
    },
    onSuccess: () => {
      invalidateZones();
      setEditingZone(null);
    },
  });

  const createErrorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create zone.'
        : null;

  const patchErrorMessage =
    patchMutation.error instanceof ApiError
      ? patchMutation.error.message
      : patchMutation.isError
        ? 'Failed to save zone.'
        : null;

  const zones = zonesQuery.data ?? [];

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Zones</h1>
        <button
          type="button"
          onClick={() => setShowAddForm(true)}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400"
        >
          Add Zone
        </button>
      </div>

      {zonesQuery.isLoading && <p className="text-sm text-slate-500">Loading zones…</p>}
      {zonesQuery.isError && <p className="text-sm text-red-400">Failed to load zones.</p>}
      {zonesQuery.data && zones.length === 0 && <p className="text-sm text-slate-500">No zones yet.</p>}

      {zones.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Center (lat, lon)</th>
              <th className="py-2 pr-4 font-medium">Radius</th>
              <th className="py-2 pr-4 font-medium">Notes</th>
            </tr>
          </thead>
          <tbody>
            {zones.map((zone) => {
              const isExpanded = expandedId === zone.id;
              return (
                <Fragment key={zone.id}>
                  <tr data-testid={`zone-row-${zone.id}`} className="border-b border-slate-900 align-top">
                    <td className="py-2 pr-4 text-slate-200">
                      <button
                        type="button"
                        onClick={() => setExpandedId(isExpanded ? null : zone.id)}
                        className="text-left text-slate-200 underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                      >
                        {zone.name}
                      </button>
                    </td>
                    <td data-testid={`zone-center-${zone.id}`} className="py-2 pr-4 font-mono text-slate-300">
                      {zone.lat}, {zone.lon}
                    </td>
                    <td data-testid={`zone-radius-${zone.id}`} className="py-2 pr-4 text-slate-300">
                      {zone.radius_m} m
                    </td>
                    <td className="py-2 pr-4 text-slate-300">{zone.notes ?? '—'}</td>
                  </tr>
                  {isExpanded && (
                    <ZoneDetail
                      zone={zone}
                      onEdit={() => setEditingZone(zone)}
                      onDeleted={() => setExpandedId(null)}
                    />
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      )}

      {showAddForm && (
        <ZoneForm
          zone={null}
          onCancel={() => {
            setShowAddForm(false);
            createMutation.reset();
          }}
          onSubmit={(input) => createMutation.mutate(input)}
          submitting={createMutation.isPending}
          submitErrorMessage={createErrorMessage}
        />
      )}

      {editingZone && (
        <ZoneForm
          zone={editingZone}
          onCancel={() => {
            setEditingZone(null);
            patchMutation.reset();
          }}
          onSubmit={(input) => patchMutation.mutate(input)}
          submitting={patchMutation.isPending}
          submitErrorMessage={patchErrorMessage}
        />
      )}

      <p className="text-xs text-slate-500">
        Deleting a zone disables (but doesn&apos;t delete) any alert rule that watches it.
      </p>
    </div>
  );
}

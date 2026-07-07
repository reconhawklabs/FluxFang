import { useState } from 'react';
import type { FormEvent } from 'react';
import type { CreateZoneInput, Zone } from '../api/zones';

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';
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
export function ZoneForm({ zone, onCancel, onSubmit, submitting, submitErrorMessage }: ZoneFormProps) {
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

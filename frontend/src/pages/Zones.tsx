// Task 9.8, redesigned per the list-pages UX cleanup (Task 5, same
// convention as `Emitters.tsx`/`Entities.tsx`): manage geofence zones — list,
// add — with each row's name linking to its own deep-linkable detail page
// (`/zones/:id`, `pages/ZoneDetailPage.tsx`), which now owns the current
// subjects (`GET /api/zones/:id`'s `emitters`/`entities`, per
// `ZoneRepo::subjects_in_zone`) and edit/delete that used to live in an
// inline expand-in-place dropdown here.
//
// A mini-map pin-drop (the brief's "nice to have") is intentionally omitted
// — YAGNI beyond the required lat/lon number inputs, which are simpler to
// test and don't need jsdom/WebGL guarding like `MapView`'s canvas does.
import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import { createZone, deleteZone, listZones, patchZone } from '../api/zones';
import type { CreateZoneInput, PatchZoneInput, Zone } from '../api/zones';
import { ZoneForm } from '../components/ZoneForm';

export default function Zones() {
  const queryClient = useQueryClient();
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
    mutationFn: ({ id, input }: { id: string; input: PatchZoneInput }) => patchZone(id, input),
    onSuccess: () => {
      invalidateZones();
      setEditingZone(null);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteZone(id),
    onSuccess: invalidateZones,
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
        ? 'Failed to update zone.'
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
              <th className="py-2 pr-2 font-medium text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {zones.map((zone) => (
              <tr key={zone.id} data-testid={`zone-row-${zone.id}`} className="border-b border-slate-900 align-top">
                <td className="py-2 pr-4 text-slate-200">
                  <Link
                    to={`/zones/${zone.id}`}
                    className="underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                  >
                    {zone.name}
                  </Link>
                </td>
                <td data-testid={`zone-center-${zone.id}`} className="py-2 pr-4 font-mono text-slate-300">
                  {zone.lat}, {zone.lon}
                </td>
                <td data-testid={`zone-radius-${zone.id}`} className="py-2 pr-4 text-slate-300">
                  {zone.radius_m} m
                </td>
                <td className="py-2 pr-4 text-slate-300">{zone.notes ?? '—'}</td>
                <td className="py-2 pr-2">
                  <div className="flex justify-end gap-2">
                    <button
                      type="button"
                      data-testid={`zone-edit-${zone.id}`}
                      onClick={() => {
                        setShowAddForm(false);
                        patchMutation.reset();
                        setEditingZone(zone);
                      }}
                      className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-slate-500 hover:text-amber-400"
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      data-testid={`zone-delete-${zone.id}`}
                      disabled={deleteMutation.isPending}
                      onClick={() => {
                        if (
                          window.confirm(
                            `Delete zone "${zone.name}"? Any alert rules watching it will be disabled, not deleted.`,
                          )
                        ) {
                          deleteMutation.mutate(zone.id);
                        }
                      }}
                      className="rounded border border-red-900 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:opacity-50"
                    >
                      Delete
                    </button>
                  </div>
                </td>
              </tr>
            ))}
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
          onSubmit={(input) =>
            patchMutation.mutate({ id: editingZone.id, input: input as PatchZoneInput })
          }
          submitting={patchMutation.isPending}
          submitErrorMessage={patchErrorMessage}
        />
      )}

      {deleteMutation.isError && (
        <p role="alert" className="text-sm text-red-400">
          Failed to delete zone.
        </p>
      )}

      <p className="text-xs text-slate-500">
        Deleting a zone disables (but doesn&apos;t delete) any alert rule that watches it.
      </p>
    </div>
  );
}

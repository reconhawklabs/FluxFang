// Dedicated zone detail page (`/zones/:id`) — replaces the old
// expand-in-place dropdown on the Zones list. Shows the zone's
// center/radius/notes and current subjects (`GET /api/zones/:id`), with
// Edit (the shared ZoneForm modal) + Delete.
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError } from "../api/client";
import { queryKeys } from "../api/queryKeys";
import { deleteZone, getZoneDetail, patchZone } from "../api/zones";
import type { CreateZoneInput, PatchZoneInput } from "../api/zones";
import { ZoneForm } from "../components/ZoneForm";

const sectionTitleClassName =
  "text-xs font-medium uppercase tracking-wide text-slate-500";
const smallButtonClassName =
  "rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50";

export default function ZoneDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [isEditing, setIsEditing] = useState(false);

  const detailQuery = useQuery({
    queryKey: [...queryKeys.zones, id],
    queryFn: () => getZoneDetail(id),
    enabled: id.length > 0,
  });

  function invalidateZones(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.zones });
  }

  const patchMutation = useMutation({
    mutationFn: (input: CreateZoneInput) =>
      patchZone(id, input as PatchZoneInput),
    onSuccess: () => {
      invalidateZones();
      setIsEditing(false);
    },
  });
  const deleteMutation = useMutation({
    mutationFn: () => deleteZone(id),
    onSuccess: () => {
      invalidateZones();
      navigate("/zones");
    },
  });

  const patchErrorMessage =
    patchMutation.error instanceof ApiError
      ? patchMutation.error.message
      : patchMutation.isError
        ? "Failed to save zone."
        : null;

  if (detailQuery.isLoading) {
    return <p className="text-sm text-slate-500">Loading zone…</p>;
  }
  if (detailQuery.isError) {
    const notFound =
      detailQuery.error instanceof ApiError && detailQuery.error.status === 404;
    return (
      <div className="space-y-3">
        <Link to="/zones" className="text-sm text-amber-400 hover:underline">
          ← Zones
        </Link>
        <p className="text-sm text-red-400">
          {notFound ? "Zone not found." : "Failed to load zone."}
        </p>
      </div>
    );
  }
  if (!detailQuery.data) {
    return (
      <div className="space-y-3">
        <Link to="/zones" className="text-sm text-amber-400 hover:underline">
          ← Zones
        </Link>
        <p className="text-sm text-red-400">Zone not found.</p>
      </div>
    );
  }
  const detail = detailQuery.data;
  const hasSubjects = detail.emitters.length > 0 || detail.entities.length > 0;

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <Link to="/zones" className="text-sm text-amber-400 hover:underline">
          ← Zones
        </Link>
        <h1 className="text-xl font-semibold text-slate-100">{detail.name}</h1>
      </div>

      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-500">Center (lat, lon)</dt>
        <dd className="font-mono text-slate-300">
          {detail.lat}, {detail.lon}
        </dd>
        <dt className="text-slate-500">Radius</dt>
        <dd className="text-slate-300">{detail.radius_m} m</dd>
        <dt className="text-slate-500">Notes</dt>
        <dd className="text-slate-300">{detail.notes ?? "—"}</dd>
      </dl>

      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Subjects in this zone</h2>
        {!hasSubjects ? (
          <p className="text-sm text-slate-500">
            No subjects currently in this zone.
          </p>
        ) : (
          <div className="space-y-3">
            {detail.entities.length > 0 && (
              <ul className="space-y-0.5 text-sm text-slate-300">
                {detail.entities.map((entity) => (
                  <li key={entity.id}>
                    <Link
                      to={`/entities/${entity.id}`}
                      className="underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                    >
                      {entity.name}
                    </Link>{" "}
                    <span className="text-slate-500">(entity)</span>
                  </li>
                ))}
              </ul>
            )}
            {detail.emitters.length > 0 && (
              <ul className="space-y-0.5 text-sm text-slate-300">
                {detail.emitters.map((emitter) => (
                  <li key={emitter.id}>
                    <Link
                      to={`/emitters/${emitter.id}`}
                      className="underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                    >
                      {emitter.name}
                    </Link>{" "}
                    <span className="text-slate-500">
                      ({emitter.type ?? "emitter"})
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </section>

      <div className="flex gap-2 border-t border-slate-800 pt-4">
        <button
          type="button"
          onClick={() => setIsEditing(true)}
          className={smallButtonClassName}
        >
          Edit
        </button>
        <button
          type="button"
          disabled={deleteMutation.isPending}
          onClick={() => {
            if (
              window.confirm(
                `Delete zone "${detail.name}"? Any alert rules watching it will be disabled, not deleted.`,
              )
            )
              deleteMutation.mutate();
          }}
          className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 hover:border-red-500 disabled:opacity-50"
        >
          {deleteMutation.isPending ? "Deleting…" : "Delete zone"}
        </button>
      </div>

      {isEditing && (
        <ZoneForm
          zone={detail}
          onCancel={() => {
            setIsEditing(false);
            patchMutation.reset();
          }}
          onSubmit={(input) => patchMutation.mutate(input)}
          submitting={patchMutation.isPending}
          submitErrorMessage={patchErrorMessage}
        />
      )}
    </div>
  );
}

// Dedicated entity detail page (`/entities/:id`) — replaces the old
// expand-in-place dropdown on the Entities list. Fetches `GET /api/entities/:id`
// (associated emitters + aggregate last-seen + recent detections), lets the
// name/notes be edited, lists/creates alert rules, and deletes the entity.
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError } from "../api/client";
import { queryKeys } from "../api/queryKeys";
import { deleteEntity, getEntityDetail, patchEntity } from "../api/entities";
import type { PatchEntityInput } from "../api/entities";
import { listAlertRules } from "../api/alertRules";
import EmissionsHeatmap from "../components/EmissionsHeatmap";
import type { HeatmapPoint } from "../components/mapData";
import { AddAlertRuleForm } from "../components/AddAlertRuleForm";

const LIVE_WINDOW_MS = 5 * 60 * 1000;
const sectionTitleClassName =
  "text-xs font-medium uppercase tracking-wide text-slate-500";
const inputClassName =
  "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none";
const smallButtonClassName =
  "rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50";

function formatTimestamp(iso: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function isRecentlySeen(iso: string | null): boolean {
  if (!iso) return false;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return false;
  return Date.now() - date.getTime() <= LIVE_WINDOW_MS;
}

export default function EntityDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [isEditing, setIsEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [notesDraft, setNotesDraft] = useState("");
  const [showAddAlert, setShowAddAlert] = useState(false);

  const detailQuery = useQuery({
    queryKey: [...queryKeys.entities, id],
    queryFn: () => getEntityDetail(id),
    enabled: id.length > 0,
  });
  const alertRulesQuery = useQuery({
    queryKey: queryKeys.alertRules,
    queryFn: listAlertRules,
  });

  const detail = detailQuery.data;

  // Seed the name/notes editor once the detail arrives.
  const [seededFor, setSeededFor] = useState<string | null>(null);
  if (detail && seededFor !== detail.id) {
    setSeededFor(detail.id);
    setNameDraft(detail.name);
    setNotesDraft(detail.notes ?? "");
  }

  function invalidateEntities(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
  }

  const patchMutation = useMutation({
    mutationFn: (body: PatchEntityInput) => patchEntity(id, body),
    onSuccess: () => {
      invalidateEntities();
      setIsEditing(false);
    },
  });
  const deleteMutation = useMutation({
    mutationFn: () => deleteEntity(id),
    onSuccess: () => {
      invalidateEntities();
      navigate("/entities");
    },
  });

  const heatmapPoints = useMemo<HeatmapPoint[]>(
    () =>
      (detail?.recent_detections ?? []).map((d) => ({
        lon: d.lon,
        lat: d.lat,
      })),
    [detail],
  );
  const rulesForEntity = (alertRulesQuery.data ?? []).filter(
    (rule) => rule.target_id === id,
  );

  function handleSaveEdit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = nameDraft.trim();
    if (trimmedName.length === 0) return;
    const trimmedNotes = notesDraft.trim();
    patchMutation.mutate({
      name: trimmedName,
      notes: trimmedNotes.length > 0 ? trimmedNotes : null,
    });
  }

  const patchErrorMessage =
    patchMutation.error instanceof ApiError
      ? patchMutation.error.message
      : patchMutation.isError
        ? "Failed to save changes."
        : null;

  if (detailQuery.isLoading) {
    return <p className="text-sm text-slate-500">Loading entity…</p>;
  }
  if (detailQuery.isError || !detail) {
    return (
      <div className="space-y-3">
        <Link to="/entities" className="text-sm text-amber-400 hover:underline">
          ← Entities
        </Link>
        <p className="text-sm text-red-400">Entity not found.</p>
      </div>
    );
  }

  const recent = isRecentlySeen(detail.last_seen);
  const dotClass =
    detail.last_seen === null
      ? "bg-slate-700"
      : recent
        ? "bg-green-500"
        : "bg-slate-500";

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <Link to="/entities" className="text-sm text-amber-400 hover:underline">
          ← Entities
        </Link>
        <div className="flex items-center gap-2">
          <span
            className={`inline-block h-2.5 w-2.5 rounded-full ${dotClass}`}
          />
          <h1 className="text-xl font-semibold text-slate-100">
            {detail.name}
          </h1>
        </div>
      </div>

      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Last seen</h2>
        <p className="text-sm text-slate-300">
          {detail.last_seen === null
            ? "Never"
            : formatTimestamp(detail.last_seen)}
        </p>
        <p className="text-xs text-slate-500">
          {detail.recent_detections.length} recent detection
          {detail.recent_detections.length === 1 ? "" : "s"}
        </p>
      </section>

      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Detection heatmap</h2>
        <p className="text-xs text-slate-500">
          Where this entity&apos;s emitters have been heard.
        </p>
        <EmissionsHeatmap points={heatmapPoints} />
      </section>

      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Emitters</h2>
        {detail.emitters.length === 0 ? (
          <p className="text-sm text-slate-500">
            No emitters associated with this entity yet.
          </p>
        ) : (
          <table className="w-full border-collapse text-left text-xs">
            <thead>
              <tr className="border-b border-slate-800 text-slate-500">
                <th className="py-1 pr-4 font-medium">Name</th>
                <th className="py-1 pr-4 font-medium">Type</th>
                <th className="py-1 pr-4 font-medium">Last Seen</th>
              </tr>
            </thead>
            <tbody>
              {detail.emitters.map((emitter) => (
                <tr key={emitter.id}>
                  <td className="py-1 pr-4 text-slate-200">
                    <Link
                      to={`/emitters/${emitter.id}`}
                      className="underline decoration-slate-600 decoration-dotted hover:text-amber-400"
                    >
                      {emitter.name}
                    </Link>
                  </td>
                  <td className="py-1 pr-4 text-slate-300">
                    {emitter.type ?? "—"}
                  </td>
                  <td className="py-1 pr-4 text-slate-300">
                    {formatTimestamp(emitter.last_seen_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="space-y-1">
        <div className="flex items-center justify-between">
          <h2 className={sectionTitleClassName}>Name / Notes</h2>
          {!isEditing && (
            <button
              type="button"
              onClick={() => setIsEditing(true)}
              className={smallButtonClassName}
            >
              Edit
            </button>
          )}
        </div>
        {isEditing ? (
          <form onSubmit={handleSaveEdit} className="space-y-2">
            <label htmlFor="entity-edit-name" className="sr-only">
              Edit name
            </label>
            <input
              id="entity-edit-name"
              type="text"
              value={nameDraft}
              onChange={(event) => setNameDraft(event.target.value)}
              className={inputClassName}
            />
            <label htmlFor="entity-edit-notes" className="sr-only">
              Edit notes
            </label>
            <textarea
              id="entity-edit-notes"
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
              <button
                type="submit"
                disabled={patchMutation.isPending}
                className={smallButtonClassName}
              >
                {patchMutation.isPending ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                onClick={() => {
                  setIsEditing(false);
                  setNameDraft(detail.name);
                  setNotesDraft(detail.notes ?? "");
                }}
                className={smallButtonClassName}
              >
                Cancel
              </button>
            </div>
          </form>
        ) : (
          <p className="text-sm text-slate-300">
            {detail.notes || "No notes."}
          </p>
        )}
      </section>

      <section className="space-y-1">
        <div className="flex items-center justify-between">
          <h2 className={sectionTitleClassName}>Alert rules</h2>
          <button
            type="button"
            onClick={() => setShowAddAlert(true)}
            className={smallButtonClassName}
          >
            Add Alert
          </button>
        </div>
        {rulesForEntity.length === 0 ? (
          <p className="text-sm text-slate-500">
            No alert rules configured for this entity yet.
          </p>
        ) : (
          <ul className="space-y-0.5 text-sm text-slate-300">
            {rulesForEntity.map((rule) => (
              <li key={rule.id}>
                {rule.name}{" "}
                <span className="text-slate-500">— {rule.trigger.on}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <div className="border-t border-slate-800 pt-4">
        <button
          type="button"
          disabled={deleteMutation.isPending}
          onClick={() => {
            if (
              window.confirm(
                `Delete ${detail.name}? Its emitters will be detached, not deleted.`,
              )
            )
              deleteMutation.mutate();
          }}
          className="rounded border border-slate-700 px-3 py-1.5 text-sm text-red-400 hover:border-red-500 disabled:opacity-50"
        >
          {deleteMutation.isPending ? "Deleting…" : "Delete entity"}
        </button>
      </div>

      {showAddAlert && (
        <AddAlertRuleForm
          entity={{ id: detail.id, name: detail.name }}
          onCancel={() => setShowAddAlert(false)}
          onCreated={() => {
            void queryClient.invalidateQueries({
              queryKey: queryKeys.alertRules,
            });
            setShowAddAlert(false);
          }}
        />
      )}
    </div>
  );
}

// Dedicated emitter detail page (`/emitters/:id`) — replaces the old
// expand-in-place dropdown on the Emitters list. Fetches the emitter by id
// (so refresh/deep-link works), plus its emissions for the detection
// heatmap, recent-emissions table, and last-known location. Reuses the
// shared badges/helpers in `components/emitterDisplay.tsx`.
import { Fragment, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError } from "../api/client";
import { queryKeys } from "../api/queryKeys";
import { createEntity, listEntities } from "../api/entities";
import type { Entity } from "../api/entities";
import {
  deleteEmitter,
  getEmitter,
  patchEmitter,
  setEmitterRule,
} from "../api/emitters";
import { listEmissions } from "../api/emissions";
import type { Emission } from "../api/emissions";
import type { Rule } from "../types/rule";
import EmissionsHeatmap from "../components/EmissionsHeatmap";
import RuleBuilder from "../components/RuleBuilder";
import type { HeatmapPoint } from "../components/mapData";
import {
  EMPTY_RULE,
  MacIdentityCell,
  RULE_EDITOR_KIND,
  TypeBadge,
  asRule,
  formatAttributeValue,
  formatTimestamp,
  isRandomizedMac,
  ruleConditions,
  ruleMatchModeLabel,
} from "../components/emitterDisplay";

const EMISSIONS_LIMIT = 500;
const RECENT_SHOWN = 10;
const NEW_ENTITY_VALUE = "__new_entity__";
const DETACH_VALUE = "__detach__";

const selectClassName =
  "rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 focus:border-amber-500 focus:outline-none";
const sectionTitleClassName =
  "text-xs font-medium uppercase tracking-wide text-slate-500";

export default function EmitterDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const emitterQuery = useQuery({
    queryKey: [...queryKeys.emitters, id],
    queryFn: () => getEmitter(id),
    enabled: id.length > 0,
  });

  const emissionsQuery = useQuery({
    queryKey: [...queryKeys.emissions, "emitter-detail", id],
    queryFn: () => {
      const p = new URLSearchParams();
      p.set("emitter_id", id);
      p.set("limit", String(EMISSIONS_LIMIT));
      return listEmissions(p);
    },
    enabled: id.length > 0,
  });

  const entitiesQuery = useQuery({
    queryKey: queryKeys.entities,
    queryFn: () => listEntities({ limit: 500 }),
  });
  const entities = useMemo(
    () => entitiesQuery.data?.items ?? [],
    [entitiesQuery.data],
  );

  const emitter = emitterQuery.data;

  const [draftRule, setDraftRule] = useState<Rule>(EMPTY_RULE);
  // Seed the rule editor once the emitter arrives (keyed by id so navigating
  // between emitters re-seeds). `seededFor` tracks which id the draft is for.
  const [seededFor, setSeededFor] = useState<string | null>(null);
  if (emitter && seededFor !== emitter.id) {
    setSeededFor(emitter.id);
    setDraftRule(asRule(emitter.match_criteria) ?? EMPTY_RULE);
  }

  function invalidateEmitter(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
  }

  const saveRuleMutation = useMutation({
    mutationFn: (rule: Rule) => setEmitterRule(id, rule),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
      void queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
    },
  });

  const patchMutation = useMutation({
    mutationFn: (body: Parameters<typeof patchEmitter>[1]) =>
      patchEmitter(id, body),
    onSuccess: invalidateEmitter,
  });

  const createAndAssociateMutation = useMutation({
    mutationFn: async (entityName: string) => {
      const entity = await createEntity({ name: entityName });
      return patchEmitter(id, { entity_id: entity.id });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.entities });
      invalidateEmitter();
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteEmitter(id),
    onSuccess: () => {
      invalidateEmitter();
      navigate("/emitters");
    },
  });

  const emissions = emissionsQuery.data?.items ?? [];
  const recent = emissions.slice(0, RECENT_SHOWN);
  const locatedEmissions = useMemo(
    () =>
      emissions.filter(
        (e): e is Emission & { lon: number; lat: number } =>
          e.lon !== null && e.lat !== null,
      ),
    [emissions],
  );
  const heatmapPoints = useMemo<HeatmapPoint[]>(
    () => locatedEmissions.map((e) => ({ lon: e.lon, lat: e.lat })),
    [locatedEmissions],
  );
  // Last-known location = the located emission with the newest observed_at.
  const lastKnown = useMemo(() => {
    if (locatedEmissions.length === 0) return null;
    return locatedEmissions.reduce((a, b) =>
      a.observed_at > b.observed_at ? a : b,
    );
  }, [locatedEmissions]);

  function handleAssociate(value: string): void {
    if (value === NEW_ENTITY_VALUE) {
      const name = window.prompt("New entity name?")?.trim() ?? "";
      if (name.length === 0) return;
      createAndAssociateMutation.mutate(name);
      return;
    }
    if (value === DETACH_VALUE) {
      patchMutation.mutate({ entity_id: null });
      return;
    }
    if (value.length === 0) return;
    patchMutation.mutate({ entity_id: value });
  }

  if (emitterQuery.isLoading) {
    return <p className="text-sm text-slate-500">Loading emitter…</p>;
  }
  if (emitterQuery.isError) {
    const notFound =
      emitterQuery.error instanceof ApiError &&
      emitterQuery.error.status === 404;
    return (
      <div className="space-y-3">
        <Link to="/emitters" className="text-sm text-amber-400 hover:underline">
          ← Emitters
        </Link>
        <p className="text-sm text-red-400">
          {notFound ? "Emitter not found." : "Failed to load emitter."}
        </p>
      </div>
    );
  }
  if (!emitter) {
    return (
      <div className="space-y-3">
        <Link to="/emitters" className="text-sm text-amber-400 hover:underline">
          ← Emitters
        </Link>
        <p className="text-sm text-red-400">Emitter not found.</p>
      </div>
    );
  }

  const conditions = ruleConditions(emitter.match_criteria);
  const attributeEntries = Object.entries(emitter.attributes ?? {});
  const entityName = emitter.entity_id
    ? (entities.find((e) => e.id === emitter.entity_id)?.name ??
      emitter.entity_id)
    : null;

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <Link to="/emitters" className="text-sm text-amber-400 hover:underline">
          ← Emitters
        </Link>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="text-xl font-semibold text-slate-100">
            {emitter.name}
          </h1>
          <TypeBadge emitter={emitter} />
        </div>
      </div>

      {/* Summary */}
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-500">Identity</dt>
        <dd>
          <MacIdentityCell emitter={emitter} />
        </dd>
        <dt className="text-slate-500">First seen</dt>
        <dd className="text-slate-300">
          {formatTimestamp(emitter.first_seen_at)}
        </dd>
        <dt className="text-slate-500">Last seen</dt>
        <dd className="text-slate-300">
          {formatTimestamp(emitter.last_seen_at)}
        </dd>
        <dt className="text-slate-500">Last-known location</dt>
        <dd className="font-mono text-slate-300">
          {lastKnown ? `${lastKnown.lat}, ${lastKnown.lon}` : "—"}
        </dd>
        <dt className="text-slate-500">Entity</dt>
        <dd className="text-slate-300">{entityName ?? "—"}</dd>
      </dl>

      {/* Entity association */}
      <div className="flex items-center gap-2">
        <label htmlFor="associate" className={sectionTitleClassName}>
          Associate to entity
        </label>
        <select
          id="associate"
          value=""
          onChange={(event) => handleAssociate(event.target.value)}
          className={selectClassName}
        >
          <option value="" disabled>
            Associate…
          </option>
          {emitter.entity_id && <option value={DETACH_VALUE}>Detach</option>}
          {entities.map((entity: Entity) => (
            <option key={entity.id} value={entity.id}>
              {entity.name}
            </option>
          ))}
          <option value={NEW_ENTITY_VALUE}>+ New entity…</option>
        </select>
      </div>

      {/* Attributes */}
      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Attributes</h2>
        {attributeEntries.length === 0 ? (
          <p className="text-sm text-slate-500">No attributes recorded.</p>
        ) : (
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
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
            onClick={() =>
              patchMutation.mutate({
                attributes: {
                  ...(emitter.attributes ?? {}),
                  randomized_mac: !isRandomizedMac(emitter.attributes ?? {}),
                },
              })
            }
            className="text-xs text-slate-500 underline decoration-dotted hover:text-amber-400"
          >
            {isRandomizedMac(emitter.attributes ?? {})
              ? "Mark as not randomized"
              : "Mark as randomized"}
          </button>
        )}
      </section>

      {/* Match rule */}
      <section className="space-y-2">
        <h2 className={sectionTitleClassName}>Match rule</h2>
        <label className="flex items-center gap-1.5 text-xs text-slate-400">
          <input
            type="checkbox"
            role="switch"
            aria-label={`Rule enabled for ${emitter.name}`}
            checked={emitter.match_enabled}
            onChange={() =>
              patchMutation.mutate({ match_enabled: !emitter.match_enabled })
            }
            className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
          />
          {emitter.match_enabled ? "Enabled" : "Disabled"}
        </label>
        {conditions.length === 0 ? (
          <p className="text-sm text-slate-500">
            No conditions — this emitter doesn&apos;t auto-attach new emissions.
          </p>
        ) : (
          <div className="text-sm text-slate-300">
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
        <div className="space-y-2 border-t border-slate-800 pt-3">
          <h3 className={sectionTitleClassName}>Edit rule</h3>
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
              className="rounded border border-amber-600 bg-amber-500/10 px-3 py-1.5 text-sm text-amber-400 hover:border-amber-500 hover:bg-amber-500/20 disabled:opacity-50"
            >
              {saveRuleMutation.isPending ? "Saving…" : "Save rule"}
            </button>
            {saveRuleMutation.isSuccess && (
              <span className="text-xs text-slate-400">
                Saved — attached {saveRuleMutation.data.attached_count} emission
                {saveRuleMutation.data.attached_count === 1 ? "" : "s"}.
              </span>
            )}
          </div>
        </div>
      </section>

      {/* Detection heatmap */}
      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Detection heatmap</h2>
        <p className="text-xs text-slate-500">
          Where this emitter has been heard.
        </p>
        {/* Larger than the default compact embed: this is the emitter's
            primary spatial view, so give it a full-width, tall canvas. */}
        <EmissionsHeatmap points={heatmapPoints} height="460px" />
      </section>

      {/* Recent emissions */}
      <section className="space-y-1">
        <h2 className={sectionTitleClassName}>Recent emissions</h2>
        {recent.length === 0 ? (
          <p className="text-sm text-slate-500">
            No emissions recorded for this emitter yet.
          </p>
        ) : (
          <table className="w-full border-collapse text-left text-xs">
            <thead>
              <tr className="border-b border-slate-800 text-slate-500">
                <th className="py-1 pr-4 font-medium">Observed At</th>
                <th className="py-1 pr-4 font-medium">BSSID</th>
                <th className="py-1 pr-4 font-medium">SSID</th>
                <th className="py-1 pr-4 font-medium">RSSI</th>
              </tr>
            </thead>
            <tbody>
              {recent.map((emission: Emission) => (
                <tr key={emission.id}>
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
      </section>

      {/* Delete */}
      <div className="border-t border-slate-800 pt-4">
        <button
          type="button"
          disabled={deleteMutation.isPending}
          onClick={() => {
            if (window.confirm("Delete this emitter?")) deleteMutation.mutate();
          }}
          className="rounded border border-slate-700 px-3 py-1.5 text-sm text-red-400 hover:border-red-500 disabled:opacity-50"
        >
          {deleteMutation.isPending ? "Deleting…" : "Delete emitter"}
        </button>
      </div>
    </div>
  );
}

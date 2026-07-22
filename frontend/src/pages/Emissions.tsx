// Task 9.4, redesigned per Phase 2 of the list-pages UX cleanup (see
// `docs/superpowers/specs/2026-07-05-list-pages-ux-cleanup-design.md`,
// "Emissions page" section): browse/filter captured emissions and assign
// them (in bulk, or one row at a time via the trailing "+") to an emitter.
//
// Top controls: a Data Source `<select>` (left) -> `data_source_id`, a
// full-width `SearchBar` (right) -> `q`, and below both a
// `StackedFilterBuilder` (kind="wifi" — the only capture kind this schema
// currently supports, see backend `fluxfang_core::catalog::catalog_for`) ->
// repeated `cond=` params via `filterState.ts`'s `conditionsToQueryParams`.
// All three combine (plus this page's own `limit`/`offset`) into one
// `GET /api/emissions` query; changing any of them resets `offset` to 0 and
// clears the row selection (a selected id from a since-changed page may no
// longer be present — see `handle*Change` below).
//
// The query is keyed off `queryKeys.emissions` (with the serialized params
// appended) so `useLiveEvents` (Task 9.1) invalidating that key on every WS
// `emission` frame refetches this page's current filter/page automatically
// — `invalidateQueries` matches by prefix.
//
// "Assign to emitter" (bulk, via row checkboxes + the toolbar button, or a
// single row via its trailing "+") opens the same modal, a `RuleBuilder`
// (Task 9.2, showPreview) whose initial rule is prefilled as `bssid eq
// <bssid>` (beacons) or `src_mac eq <src_mac>` (probe requests) — the same
// default rule the backend itself would build from `from_emission_id` (see
// `fluxfang-api::emitters`'s `resolve_match_criteria`), just built
// client-side so it's visible/editable in the modal before submitting.
// Submitting calls `POST /api/emitters` with `{name, type, match_criteria}`
// and surfaces the returned `attached_count`.
//
// Mass-select ("Delete selected"/"Clear All Emissions") uses the shared
// `useRowSelection`/`SelectionToolbar` (Phase 2) against
// `bulkDeleteEmissions`/`clearEmissions`.
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, api } from "../api/client";
import type { CachedEmission } from "../api/client";
import {
  formatObservedAt,
  payloadRecord,
  payloadText,
  payloadTextAny,
} from "../lib/emissionPayload";
import { queryKeys } from "../api/queryKeys";
import { useConfig } from "../hooks/useConfig";
import type { Emission } from "../api/emissions";
import {
  bulkDeleteEmissions,
  clearEmissions,
  listEmissions,
  listSensorIds,
} from "../api/emissions";
import { createEmitter, listEmitters, listEmitterTypes } from "../api/emitters";
import type { EmitterType } from "../api/emitters";
import { isEmittingSource, listDataSources } from "../api/dataSources";
import type { DataSource } from "../api/dataSources";
import Pagination from "../components/Pagination";
import RuleBuilder from "../components/RuleBuilder";
import SearchBar from "../components/SearchBar";
import SelectionToolbar from "../components/SelectionToolbar";
import { SortableTh, type SortDir } from "../components/SortableTh";
import StackedFilterBuilder from "../components/StackedFilterBuilder";
import { conditionsToQueryParams } from "../components/filterState";
import { useRowSelection } from "../hooks/useRowSelection";
import type { Condition, Rule } from "../types/rule";

const DEFAULT_LIMIT = 50;

const inputClassName =
  "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none";
const labelClassName =
  "block text-xs font-medium uppercase tracking-wide text-slate-500";
const selectClassName =
  "rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none";

/** A `DataSource`'s label in the filter dropdown: kind plus whatever
 * identifies it (its interface, when set — wifi monitor sources — or
 * otherwise its mode, e.g. a gpsd/serial gps source with no `interface`). */
function dataSourceLabel(dataSource: DataSource): string {
  return dataSource.interface
    ? `${dataSource.kind} (${dataSource.interface})`
    : `${dataSource.kind} (${dataSource.mode})`;
}

/** The default rule a fresh "assign to emitter" modal opens with: `bssid eq
 * <bssid>` for a beacon (has `payload.bssid`) or `src_mac eq <src_mac>` for
 * a probe request (has `payload.src_mac`, no `bssid`) — mirrors the
 * backend's own `from_emission_id` default-rule derivation (see module doc
 * comment), just computed client-side. Falls back to an empty rule (user
 * picks fields manually via `RuleBuilder`) if neither key is a non-empty
 * string. */
function defaultRuleFor(emission: Emission): Rule {
  const bssid = emission.payload.bssid;
  if (typeof bssid === "string" && bssid.length > 0) {
    return {
      match: "all",
      conditions: [{ field: "bssid", op: "eq", value: bssid }],
    };
  }
  const srcMac = emission.payload.src_mac;
  if (typeof srcMac === "string" && srcMac.length > 0) {
    return {
      match: "all",
      conditions: [{ field: "src_mac", op: "eq", value: srcMac }],
    };
  }
  return { match: "all", conditions: [] };
}

/** Sentinel `<select>` value for the escape-hatch "Other (custom)…" option
 * — never a real `EmitterType.key` (those come from the backend's
 * `emitter_type` enum, which doesn't use double-underscore keys). Selecting
 * it reveals a free-text input and, on submit, sends that text as `type`
 * with `emitter_type` omitted (kept `null` server-side), same as today's
 * free-text field. */
const OTHER_TYPE_VALUE = "__other__";

interface AssignModalProps {
  /** The emission the default rule is derived from — for a bulk assign,
   * list order's first selected row (not necessarily click order); for a
   * per-row quick-assign, that row itself. */
  seedEmission: Emission;
  selectedCount: number;
  onCancel: () => void;
  onAssigned: (attachedCount: number) => void;
}

function AssignModal({
  seedEmission,
  selectedCount,
  onCancel,
  onAssigned,
}: AssignModalProps) {
  const [name, setName] = useState("");
  // Which `<select>` option is chosen: an `EmitterType.key` (a known type,
  // e.g. "wifi_access_point") or `OTHER_TYPE_VALUE` (the custom-text escape
  // hatch). `''` means "not yet chosen" — the select falls back to the
  // first fetched option (or `OTHER_TYPE_VALUE` if the list is empty/still
  // loading) below, so the field always has a sensible selection without an
  // effect.
  const [typeSelection, setTypeSelection] = useState("");
  const [customType, setCustomType] = useState("");
  const [rule, setRule] = useState<Rule>(() => defaultRuleFor(seedEmission));

  // The emitter-types dropdown is scoped to this emission's `kind` (e.g.
  // "wifi") — mirrors `RuleBuilder`'s own `useCatalog(kind)` fetch below,
  // just against the emitter-types endpoint instead of the field catalog.
  const emitterTypesQuery = useQuery({
    queryKey: queryKeys.emitterTypes(seedEmission.kind),
    queryFn: () => listEmitterTypes(seedEmission.kind),
  });
  const emitterTypes = useMemo(
    () => emitterTypesQuery.data ?? [],
    [emitterTypesQuery.data],
  );

  const effectiveTypeSelection =
    typeSelection.length > 0
      ? typeSelection
      : emitterTypes.length > 0
        ? emitterTypes[0].key
        : OTHER_TYPE_VALUE;
  const isOtherSelected = effectiveTypeSelection === OTHER_TYPE_VALUE;

  const createMutation = useMutation({
    mutationFn: createEmitter,
    onSuccess: (result) => onAssigned(result.attached_count),
  });

  const errorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? "Failed to create emitter."
        : null;

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();

    if (isOtherSelected) {
      const trimmedType = customType.trim();
      createMutation.mutate({
        name: name.trim(),
        type: trimmedType.length > 0 ? trimmedType : undefined,
        match_criteria: rule,
      });
      return;
    }

    const selected = emitterTypes.find(
      (entry: EmitterType) => entry.key === effectiveTypeSelection,
    );
    createMutation.mutate({
      name: name.trim(),
      type: selected?.label,
      emitter_type: selected?.key,
      match_criteria: rule,
    });
  }

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-lg space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">
          Assign {selectedCount} emission{selectedCount === 1 ? "" : "s"} to
          emitter
        </h2>

        <div className="space-y-1">
          <label htmlFor="emitter-name" className={labelClassName}>
            Name
          </label>
          <input
            id="emitter-name"
            type="text"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="emitter-type" className={labelClassName}>
            Type
          </label>
          <select
            id="emitter-type"
            value={effectiveTypeSelection}
            onChange={(event) => setTypeSelection(event.target.value)}
            className={inputClassName}
          >
            {emitterTypes.map((entry: EmitterType) => (
              <option key={entry.key} value={entry.key}>
                {entry.label}
              </option>
            ))}
            <option value={OTHER_TYPE_VALUE}>Other (custom)…</option>
          </select>
        </div>

        {isOtherSelected && (
          <div className="space-y-1">
            <label htmlFor="emitter-type-custom" className={labelClassName}>
              Custom type (optional)
            </label>
            <input
              id="emitter-type-custom"
              type="text"
              value={customType}
              onChange={(event) => setCustomType(event.target.value)}
              className={inputClassName}
            />
          </div>
        )}

        <div className="space-y-1">
          <span className={labelClassName}>Match rule</span>
          <RuleBuilder
            kind="wifi"
            value={rule}
            onChange={setRule}
            showPreview
          />
        </div>

        {errorMessage && (
          <p role="alert" className="text-sm text-red-400">
            {errorMessage}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createMutation.isPending || name.trim().length === 0}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {createMutation.isPending ? "Assigning…" : "Assign"}
          </button>
        </div>
      </form>
    </div>
  );
}

/** What the assign modal is currently open for — a bulk assign of every
 * checked row (`seedEmission` is list order's first selected row), or a
 * single row's own trailing "+" (`seedEmission` is that row, regardless of
 * whether it's checked). Kept as one piece of state (rather than two
 * booleans) so the two flows can never both render a modal at once. */
type AssignTarget = { mode: "bulk" } | { mode: "single"; emission: Emission };

/** The Emissions page shown on a Sensor node: a Sensor has no local
 * `/api/emissions` browse/filter/assign backend (that's standalone-only —
 * emitters/assignment live on the standalone node it forwards to), so this
 * renders a compact read-only list of what's actually stored locally: the
 * forwarding cache (`GET /api/cached-emissions`), same `kind`/delivered
 * shape `SensorDashboard`'s "Recent captures" section shows. */
function CachedEmissionsView() {
  const cached = useQuery({
    queryKey: [...queryKeys.cachedEmissions, 100],
    queryFn: () => api.cachedEmissions(100),
    refetchInterval: 4000,
  });
  const rows = cached.data ?? [];

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold text-slate-100">Emissions</h1>
      {cached.isLoading && (
        <p className="text-sm text-slate-500">Loading emissions…</p>
      )}
      {cached.isError && (
        <p className="text-sm text-red-400">Failed to load emissions.</p>
      )}
      {cached.data && rows.length === 0 && (
        <p className="text-sm text-slate-500">No captures yet.</p>
      )}
      {rows.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-left text-sm">
            <thead>
              <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
                <th className="py-2 pr-4 font-medium">Kind</th>
                <th className="py-2 pr-4 font-medium">Observed At</th>
                <th className="py-2 pr-4 font-medium">BSSID</th>
                <th className="py-2 pr-4 font-medium">Src MAC</th>
                <th className="py-2 pr-4 font-medium">SSID/Name</th>
                <th className="py-2 pr-4 font-medium">Ch/Freq</th>
                <th className="py-2 pr-4 font-medium">RSSI</th>
                <th className="py-2 pr-2 font-medium">Status</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row: CachedEmission) => {
                const payload = payloadRecord(row.payload);
                return (
                  <tr
                    key={row.id}
                    data-testid={`cached-emission-row-${row.id}`}
                    className="border-b border-slate-900 align-top"
                  >
                    <td className="py-2 pr-4 font-mono text-slate-300">{row.kind}</td>
                    <td className="py-2 pr-4 text-slate-300">
                      {formatObservedAt(row.observed_at)}
                    </td>
                    <td className="py-2 pr-4 font-mono text-slate-300">
                      {payloadText(payload, "bssid")}
                    </td>
                    <td className="py-2 pr-4 font-mono text-slate-300">
                      {payloadTextAny(payload, ["src_mac", "address"])}
                    </td>
                    <td className="py-2 pr-4 text-slate-300">
                      {payloadTextAny(payload, ["ssid", "name"])}
                    </td>
                    <td className="py-2 pr-4 text-slate-300">
                      {payloadTextAny(payload, ["channel", "frequency"])}
                    </td>
                    <td className="py-2 pr-4 font-mono text-slate-300">
                      {row.signal_strength ?? "—"}
                    </td>
                    <td
                      className={`py-2 pr-2 ${row.delivered ? "text-emerald-400" : "text-amber-400"}`}
                    >
                      {row.delivered ? "delivered" : "pending"}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// Role gate: mirrors `Dashboard.tsx`'s pattern — `Emissions` itself only
// ever calls `useConfig`, so its hook count stays stable across the
// "config loading" -> "role known" transition; the standalone body lives in
// its own child component (`StandaloneEmissions`) instead of being inlined
// after a conditional return.
export default function Emissions() {
  const { data: config } = useConfig();
  if (config?.role === "sensor") return <CachedEmissionsView />;
  return <StandaloneEmissions />;
}

function StandaloneEmissions() {
  const queryClient = useQueryClient();
  const [dataSourceId, setDataSourceId] = useState("");
  const [sensorId, setSensorId] = useState("");
  const [q, setQ] = useState("");
  const [conditions, setConditions] = useState<Condition[]>([]);
  const [limit, setLimit] = useState<number>(DEFAULT_LIMIT);
  const [offset, setOffset] = useState(0);
  const [sortKey, setSortKey] = useState<string>("observed_at");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const [assignTarget, setAssignTarget] = useState<AssignTarget | null>(null);
  const [assignedMessage, setAssignedMessage] = useState<string | null>(null);

  const queryParams = useMemo(() => {
    const params = conditionsToQueryParams(conditions);
    const trimmedQ = q.trim();
    if (trimmedQ.length > 0) params.set("q", trimmedQ);
    if (dataSourceId.length > 0) params.set("data_source_id", dataSourceId);
    if (sensorId.length > 0) params.set("sensor_id", sensorId);
    params.set("limit", String(limit));
    params.set("offset", String(offset));
    params.set("sort", sortKey);
    params.set("dir", sortDir);
    return params;
  }, [conditions, q, dataSourceId, sensorId, limit, offset, sortKey, sortDir]);

  const emissionsQuery = useQuery({
    queryKey: [...queryKeys.emissions, queryParams.toString()],
    queryFn: () => listEmissions(queryParams),
  });

  const dataSourcesQuery = useQuery({
    queryKey: queryKeys.dataSources,
    queryFn: listDataSources,
  });

  // Distinct sensor_ids present in the data (incl. "local") for the per-sensor
  // filter — see `GET /api/emissions/sensor-ids`.
  const sensorIdsQuery = useQuery({
    queryKey: queryKeys.sensorIds,
    queryFn: listSensorIds,
  });

  // Resolves an emission's `emitter_id` to a display name. Not itself
  // invalidated by `useLiveEvents` (emitters aren't touched by a plain
  // emission frame), but this page's own "assign to emitter" mutation
  // invalidates it below, and `queryKeys.emitters` is still the correct key
  // to key this query off per the registry.
  // Interim `{limit: 500}` cap — `GET /api/emitters` now returns a
  // paginated `{items, total}` envelope; this lookup map just needs "every
  // emitter's name," so 500 keeps today's coverage without adding
  // pagination here (a later redesign phase).
  const emittersQuery = useQuery({
    queryKey: queryKeys.emitters,
    queryFn: () => listEmitters({ limit: 500 }),
  });

  const emitterNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const emitter of emittersQuery.data?.items ?? [])
      map.set(emitter.id, emitter.name);
    return map;
  }, [emittersQuery.data]);

  const items = emissionsQuery.data?.items ?? [];
  const total = emissionsQuery.data?.total ?? 0;
  const itemIds = items.map((emission) => emission.id);
  const selection = useRowSelection(itemIds);

  function resetToFirstPage(): void {
    setOffset(0);
    selection.clear();
  }

  function handleDataSourceChange(next: string): void {
    setDataSourceId(next);
    resetToFirstPage();
  }

  function handleSensorIdChange(next: string): void {
    setSensorId(next);
    resetToFirstPage();
  }

  function handleSearchChange(next: string): void {
    setQ(next);
    resetToFirstPage();
  }

  function handleConditionsChange(next: Condition[]): void {
    setConditions(next);
    resetToFirstPage();
  }

  function handlePaginationChange(nextLimit: number, nextOffset: number): void {
    setLimit(nextLimit);
    setOffset(nextOffset);
    selection.clear();
  }

  function handleSort(key: string): void {
    if (sortKey === key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
    resetToFirstPage();
  }

  // List order's first selected row — deterministic regardless of click
  // order — is what a bulk assign's default rule is derived from.
  const bulkSeedEmission = items.find((emission) =>
    selection.selected.has(emission.id),
  );

  const modalSeedEmission =
    assignTarget?.mode === "single"
      ? assignTarget.emission
      : assignTarget?.mode === "bulk"
        ? bulkSeedEmission
        : undefined;
  const modalSelectedCount =
    assignTarget?.mode === "single" ? 1 : selection.selected.size;

  function handleAssigned(attachedCount: number): void {
    const wasBulk = assignTarget?.mode === "bulk";
    setAssignTarget(null);
    if (wasBulk) selection.clear();
    setAssignedMessage(
      `Assigned ${attachedCount} emission${attachedCount === 1 ? "" : "s"}.`,
    );
    void queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
    void queryClient.invalidateQueries({ queryKey: queryKeys.emitters });
  }

  const bulkDeleteMutation = useMutation({
    mutationFn: bulkDeleteEmissions,
    onSuccess: () => {
      selection.clear();
      void queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
    },
  });

  const clearAllMutation = useMutation({
    mutationFn: clearEmissions,
    onSuccess: () => {
      selection.clear();
      void queryClient.invalidateQueries({ queryKey: queryKeys.emissions });
    },
  });

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-100">Emissions</h1>
        {assignedMessage && (
          <p role="status" className="text-sm text-amber-400">
            {assignedMessage}
          </p>
        )}
      </div>

      <div className="flex items-center gap-3">
        <label htmlFor="emissions-data-source" className="sr-only">
          Data source
        </label>
        <select
          id="emissions-data-source"
          value={dataSourceId}
          onChange={(event) => handleDataSourceChange(event.target.value)}
          className={selectClassName}
        >
          <option value="">All data sources</option>
          {(dataSourcesQuery.data ?? [])
            .filter(isEmittingSource)
            .map((dataSource) => (
              <option key={dataSource.id} value={dataSource.id}>
                {dataSourceLabel(dataSource)}
              </option>
            ))}
        </select>

        <label htmlFor="emissions-sensor" className="sr-only">
          Sensor
        </label>
        <select
          id="emissions-sensor"
          value={sensorId}
          onChange={(event) => handleSensorIdChange(event.target.value)}
          className={selectClassName}
        >
          <option value="">All sensors</option>
          {(sensorIdsQuery.data ?? []).map((sid) => (
            <option key={sid} value={sid}>
              {sid}
            </option>
          ))}
        </select>

        <SearchBar
          value={q}
          onChange={handleSearchChange}
          placeholder="Search emissions…"
        />
      </div>

      <StackedFilterBuilder
        kind="wifi"
        value={conditions}
        onChange={handleConditionsChange}
      />

      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <button
            type="button"
            disabled={selection.selected.size === 0}
            onClick={() => setAssignTarget({ mode: "bulk" })}
            className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Assign to emitter ({selection.selected.size})
          </button>
          <SelectionToolbar
            selectedCount={selection.selected.size}
            onDeleteSelected={() =>
              bulkDeleteMutation.mutate(Array.from(selection.selected))
            }
            onClearAll={() => clearAllMutation.mutate()}
            itemLabelPlural="Emissions"
          />
        </div>
        <p data-testid="emissions-total" className="text-sm text-slate-400">
          {total} emission{total === 1 ? "" : "s"}
        </p>
      </div>

      {emissionsQuery.isLoading && (
        <p className="text-sm text-slate-500">Loading emissions…</p>
      )}
      {emissionsQuery.isError && (
        <p className="text-sm text-red-400">Failed to load emissions.</p>
      )}

      {items.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-2 font-medium">
                <input
                  type="checkbox"
                  aria-label="Select all emissions on this page"
                  checked={selection.allSelected}
                  onChange={() => selection.toggleAll(itemIds)}
                  className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                />
              </th>
              <SortableTh
                label="Observed At"
                sortKey="observed_at"
                activeKey={sortKey}
                activeDir={sortDir}
                onSort={handleSort}
              />
              <th className="py-2 pr-4 font-medium">BSSID</th>
              <th className="py-2 pr-4 font-medium">Src MAC</th>
              <th className="py-2 pr-4 font-medium">SSID/Name</th>
              <th className="py-2 pr-4 font-medium">Channel/Frequency</th>
              <SortableTh
                label="RSSI"
                sortKey="rssi"
                activeKey={sortKey}
                activeDir={sortDir}
                onSort={handleSort}
              />
              <th className="py-2 pr-4 font-medium">Emitter</th>
              <th className="py-2 pr-2 font-medium" />
            </tr>
          </thead>
          <tbody>
            {items.map((emission) => (
              <tr
                key={emission.id}
                data-testid={`emission-row-${emission.id}`}
                className="border-b border-slate-900 align-top"
              >
                <td className="py-2 pr-2">
                  <input
                    type="checkbox"
                    aria-label={`Select emission ${emission.id}`}
                    checked={selection.selected.has(emission.id)}
                    onChange={() => selection.toggle(emission.id)}
                    className="h-4 w-4 rounded border-slate-700 bg-slate-950 text-amber-500 focus:ring-amber-500"
                  />
                </td>
                <td className="py-2 pr-4 text-slate-300">
                  {formatObservedAt(emission.observed_at)}
                </td>
                <td className="py-2 pr-4 font-mono text-slate-300">
                  {payloadText(emission.payload, "bssid")}
                </td>
                <td
                  data-testid="emission-src-mac"
                  className="py-2 pr-4 font-mono text-slate-300"
                >
                  {payloadTextAny(emission.payload, ["src_mac", "address"])}
                </td>
                <td className="py-2 pr-4 text-slate-300">
                  {payloadTextAny(emission.payload, ["ssid", "name"])}
                </td>
                <td className="py-2 pr-4 text-slate-300">
                  {payloadTextAny(emission.payload, ["channel", "frequency"])}
                </td>
                <td className="py-2 pr-4 font-mono text-slate-300">
                  {emission.signal_strength ?? "—"}
                </td>
                <td className="py-2 pr-4 text-slate-300">
                  {emission.emitter_id
                    ? (emitterNameById.get(emission.emitter_id) ?? "—")
                    : "—"}
                </td>
                <td className="py-2 pr-2">
                  <button
                    type="button"
                    aria-label={`Quick-assign emission ${emission.id} to emitter`}
                    onClick={() =>
                      setAssignTarget({ mode: "single", emission })
                    }
                    className="rounded border border-slate-700 px-2 py-0.5 text-sm text-slate-300 transition hover:border-amber-500 hover:text-amber-400"
                  >
                    +
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {emissionsQuery.data && items.length === 0 && (
        <p className="text-sm text-slate-500">
          No emissions match this filter.
        </p>
      )}

      <Pagination
        total={total}
        limit={limit}
        offset={offset}
        onChange={handlePaginationChange}
      />

      {assignTarget && modalSeedEmission && (
        <AssignModal
          seedEmission={modalSeedEmission}
          selectedCount={modalSelectedCount}
          onCancel={() => setAssignTarget(null)}
          onAssigned={handleAssigned}
        />
      )}
    </div>
  );
}

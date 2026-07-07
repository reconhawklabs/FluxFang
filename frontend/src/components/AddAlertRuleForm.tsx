// Extracted from `pages/Entities.tsx` (Task 4, entity detail page) so both
// the Entities list's former inline expand and the new `/entities/:id`
// detail page can share the same "add alert" form without duplicating it.
//
// The per-entity "add alert" form: name, trigger-type dropdown (revealing a
// zone picker only for a zone trigger), an optional content-match
// `RuleBuilder`, and an alert-method multi-select. Submits `POST
// /api/alert-rules` with `target_type: "entity"`/`target_id: entity.id`.
import { useState } from "react";
import type { ChangeEvent, FormEvent } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { ApiError } from "../api/client";
import { queryKeys } from "../api/queryKeys";
import { listZones } from "../api/zones";
import { listAlertMethods } from "../api/alertMethods";
import { createAlertRule } from "../api/alertRules";
import type {
  AlertRuleTrigger,
  AlertRuleTriggerOn,
  CreateAlertRuleInput,
} from "../api/alertRules";
import RuleBuilder from "./RuleBuilder";
import type { Rule } from "../types/rule";

const EMPTY_RULE: Rule = { match: "all", conditions: [] };
const inputClassName =
  "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none";
const labelClassName =
  "block text-xs font-medium uppercase tracking-wide text-slate-500";
const cancelButtonClassName =
  "rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100";
const submitButtonClassName =
  "rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50";

const TRIGGER_OPTIONS: { value: AlertRuleTriggerOn; label: string }[] = [
  { value: "detected", label: "When detected" },
  { value: "enters_zone", label: "When enters zone" },
  { value: "leaves_zone", label: "When leaves zone" },
];

interface AddAlertRuleFormProps {
  entity: { id: string; name: string };
  onCancel: () => void;
  onCreated: () => void;
}

export function AddAlertRuleForm({
  entity,
  onCancel,
  onCreated,
}: AddAlertRuleFormProps) {
  const [name, setName] = useState("");
  const [on, setOn] = useState<AlertRuleTriggerOn>("detected");
  const [zoneId, setZoneId] = useState("");
  const [matchContent, setMatchContent] = useState(false);
  const [contentMatch, setContentMatch] = useState<Rule>(EMPTY_RULE);
  const [methodIds, setMethodIds] = useState<string[]>([]);

  const isZoneTrigger = on === "enters_zone" || on === "leaves_zone";

  const zonesQuery = useQuery({
    queryKey: queryKeys.zones,
    queryFn: listZones,
  });
  const methodsQuery = useQuery({
    queryKey: queryKeys.alertMethods,
    queryFn: listAlertMethods,
  });

  const createMutation = useMutation({
    mutationFn: (input: CreateAlertRuleInput) => createAlertRule(input),
    onSuccess: onCreated,
  });

  function handleTriggerChange(event: ChangeEvent<HTMLSelectElement>): void {
    const next = event.target.value as AlertRuleTriggerOn;
    setOn(next);
    if (next === "detected") setZoneId("");
  }

  function toggleMethod(id: string, checked: boolean): void {
    setMethodIds((prev) =>
      checked ? [...prev, id] : prev.filter((existing) => existing !== id),
    );
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
      target_type: "entity",
      target_id: entity.id,
      trigger,
      method_ids: methodIds,
    });
  }

  const errorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? "Failed to create alert rule."
        : null;

  const methods = methodsQuery.data ?? [];

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="max-h-[90vh] w-full max-w-lg space-y-4 overflow-y-auto rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">
          Add Alert for {entity.name}
        </h2>

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
          <select
            id="alert-rule-trigger"
            value={on}
            onChange={handleTriggerChange}
            className={inputClassName}
          >
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
            {zonesQuery.isError && (
              <p className="text-xs text-red-400">Failed to load zones.</p>
            )}
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
          {matchContent && (
            <RuleBuilder
              kind="wifi"
              value={contentMatch}
              onChange={setContentMatch}
            />
          )}
        </div>

        <div className="space-y-1">
          <span className={labelClassName}>Alert methods</span>
          {methodsQuery.isLoading && (
            <p className="text-sm text-slate-500">Loading alert methods…</p>
          )}
          {methodsQuery.isError && (
            <p className="text-sm text-red-400">
              Failed to load alert methods.
            </p>
          )}
          {methodsQuery.data && methods.length === 0 && (
            <p className="text-sm text-slate-500">
              No alert methods configured yet — add one on the Alerts page
              first.
            </p>
          )}
          {methods.length > 0 && (
            <div className="space-y-1">
              {methods.map((method) => (
                <label
                  key={method.id}
                  className="flex items-center gap-2 text-sm text-slate-300"
                >
                  <input
                    type="checkbox"
                    checked={methodIds.includes(method.id)}
                    onChange={(event) =>
                      toggleMethod(method.id, event.target.checked)
                    }
                  />
                  {method.name}{" "}
                  <span className="text-slate-500">({method.type})</span>
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
          <button
            type="button"
            onClick={onCancel}
            className={cancelButtonClassName}
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createMutation.isPending}
            className={submitButtonClassName}
          >
            {createMutation.isPending ? "Creating…" : "Create Alert Rule"}
          </button>
        </div>
      </form>
    </div>
  );
}

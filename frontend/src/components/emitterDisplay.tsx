// Shared emitter presentation helpers + badges, used by both the Emitters
// list page (`pages/Emitters.tsx`) and the Emitter detail page
// (`pages/EmitterDetailPage.tsx`). Extracted so the two render identical
// type badges / MAC cells / rule descriptions from one copy.
import type { Emitter } from "../api/emitters";
import type { Condition, Rule } from "../types/rule";

/** The single capture `kind` whose catalog (`GET /api/catalog/:kind`) drives
 * the expanded-row rule editor's field/operator dropdowns — wifi is the only
 * kind this schema supports today, same hardcoded assumption `Emissions.tsx`
 * makes for its `StackedFilterBuilder`/`RuleBuilder`. */
export const RULE_EDITOR_KIND = "wifi";

/** Seed for the rule editor when an emitter has no well-formed rule yet
 * (`match_criteria` was `{}`) — an empty ALL-match rule the user adds
 * conditions to. */
export const EMPTY_RULE: Rule = { match: "all", conditions: [] };

/** Full, readable timestamp (used inside the expanded detail's "Recent
 * emissions" table, which has room to spare) — unlike `formatCompact`
 * below, no attempt is made to keep this to a fixed width. */
export function formatTimestamp(iso: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** Compact one-line datetime for the row's First/Last seen columns —
 * `MM/DD HH:mm`, 24-hour, no AM/PM suffix to wrap onto a second line (per
 * the design doc's "Compact rows" section). */
export function formatCompact(iso: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const mm = String(date.getMonth() + 1).padStart(2, "0");
  const dd = String(date.getDate()).padStart(2, "0");
  const hh = String(date.getHours()).padStart(2, "0");
  const min = String(date.getMinutes()).padStart(2, "0");
  return `${mm}/${dd} ${hh}:${min}`;
}

/** `match_criteria` comes back from the backend as untyped
 * `serde_json::Value` (see `Emitter.match_criteria`'s doc comment) — an
 * emitter created with no rule at all persists `{}`, which doesn't satisfy
 * the `Rule` shape (`conditions` absent). This narrows defensively rather
 * than assuming every row has a well-formed `Rule`. */
export function asRule(matchCriteria: unknown): Rule | null {
  if (!matchCriteria || typeof matchCriteria !== "object") return null;
  const conditions = (matchCriteria as { conditions?: unknown }).conditions;
  if (!Array.isArray(conditions)) return null;
  return matchCriteria as Rule;
}

function formatConditionValue(value: unknown): string {
  if (Array.isArray(value))
    return value.map((entry) => String(entry)).join(", ");
  return String(value);
}

function formatCondition(condition: Condition): string {
  return `${condition.field} ${condition.op} ${formatConditionValue(condition.value)}`;
}

/** Full, readable rule description for the expanded detail panel — one
 * condition per line (via the caller's `<ul>`), plus the match mode spelled
 * out separately by `ruleMatchModeLabel`. */
export function ruleConditions(matchCriteria: unknown): string[] {
  const rule = asRule(matchCriteria);
  if (!rule) return [];
  return rule.conditions.map(formatCondition);
}

export function ruleMatchModeLabel(matchCriteria: unknown): string {
  const rule = asRule(matchCriteria);
  return rule?.match === "any" ? "ANY" : "ALL";
}

/** Reads a string attribute out of an emitter's `attributes` bag (Phase A
 * backend's `emitter.attributes jsonb`) — defensive, since `attributes`'
 * shape depends on `emitter_type` and older/plain emitters carry `{}`. */
export function attributeText(
  attributes: Record<string, unknown>,
  key: string,
): string | null {
  const value = attributes[key];
  return typeof value === "string" ? value : null;
}

export function isRandomizedMac(attributes: Record<string, unknown>): boolean {
  return attributes.randomized_mac === true;
}

/** The five MAC persistence classes the backend assigns
 * (`fluxfang_core::classify::MacPersistence`), most- to least-persistent. */
export const MAC_PERSISTENCE_CLASSES = [
  "stable",
  "per_network",
  "session",
  "ephemeral",
  "unlinkable",
] as const;

export type MacPersistence = (typeof MAC_PERSISTENCE_CLASSES)[number];

/** Human-readable blurb per class, for the retention dropdown and the badge
 * tooltip. Mirrors the doc comments on the Rust enum. */
export const MAC_PERSISTENCE_LABELS: Record<MacPersistence, string> = {
  stable: "Stable — real vendor MAC, never randomized",
  per_network: "Per-network — randomized, persists per network for months",
  session: "Session — randomized, persists until the device reboots",
  ephemeral: "Ephemeral — randomized, rotates every few minutes",
  unlinkable: "Unlinkable — randomized, cannot be correlated at all",
};

/** Options for the `mac_persistence` filter dropdown on the Emitters and
 * Emissions pages, in menu order. The values must stay in step with
 * `fluxfang_core::classify::PERSISTENCE_FILTER_TOKENS` — the backend
 * rejects anything else with a 400 rather than matching nothing.
 *
 * The two badges come first (they're what the table shows), then the exact
 * classes for when badge granularity isn't enough. */
export const MAC_PERSISTENCE_FILTER_OPTIONS: ReadonlyArray<{
  value: string;
  label: string;
}> = [
  { value: "randomized", label: "Randomized — short-lived" },
  { value: "randomized-longterm", label: "Randomized — long-term" },
  { value: "stable", label: "Class: stable" },
  { value: "per_network", label: "Class: per-network" },
  { value: "session", label: "Class: session" },
  { value: "ephemeral", label: "Class: ephemeral" },
  { value: "unlinkable", label: "Class: unlinkable" },
];

/** The badge an emitter shows for its persistence class, or `null` for
 * none. Mirrors `MacPersistence::badge` on the backend: the two classes
 * that persist long enough to be worth tracking (`per_network`, `session`)
 * read "randomized-longterm", the short-lived ones read "randomized".
 *
 * Falls back to the legacy `randomized_mac` boolean for emitters
 * classified before `mac_persistence` existed, which can't be resolved any
 * finer than "randomized". */
export function macPersistenceBadge(
  attributes: Record<string, unknown>,
): "randomized" | "randomized-longterm" | null {
  const cls = attributes.mac_persistence;
  if (cls === "per_network" || cls === "session") return "randomized-longterm";
  if (cls === "ephemeral" || cls === "unlinkable") return "randomized";
  if (cls === "stable") return null;
  return isRandomizedMac(attributes) ? "randomized" : null;
}

/** Renders any attribute value as text for the expanded panel's full
 * key/value dump — most values here are strings/booleans (`ssid`,
 * `bssid`, `src_mac`, `randomized_mac`), but this stays permissive for
 * whatever future classifier fields show up. */
export function formatAttributeValue(value: unknown): string {
  if (
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return String(value);
  }
  return JSON.stringify(value);
}

/** The "type badge" (design doc's Frontend section): `type_label` when the
 * emitter is auto-classified (e.g. "WiFi Access Point"), falling back to the
 * free-text `type` a manually-created emitter carries, and finally "—" for
 * neither. Single-line badge — no attributes rendered alongside it (those
 * live in their own column now). */
export function TypeBadge({ emitter }: { emitter: Emitter }) {
  const label = emitter.type_label ?? emitter.type;
  if (!label) return <span className="text-slate-500">—</span>;
  return (
    <span
      data-testid={`emitter-type-badge-${emitter.id}`}
      className="inline-block whitespace-nowrap rounded bg-slate-800 px-2 py-0.5 text-xs font-medium text-slate-200"
    >
      {label}
    </span>
  );
}

/** MAC/Identity column (design doc's "Compact rows" section) — the
 * emitter's identifying MAC/BSSID (`attributes.bssid` for an access point,
 * `attributes.src_mac` for a client), monospace, plus a compact
 * "randomized" badge inline (not stacked) when `attributes.randomized_mac`
 * is set. A plain/unclassified emitter with neither key renders "—". */
export function MacIdentityCell({ emitter }: { emitter: Emitter }) {
  const attributes = emitter.attributes ?? {};
  const mac =
    attributeText(attributes, "bssid") ??
    attributeText(attributes, "src_mac") ??
    attributeText(attributes, "address");

  if (!mac) return <span className="text-slate-500">—</span>;

  const badge = macPersistenceBadge(attributes);
  const cls = attributes.mac_persistence;
  // A long-term address is a *tracking* signal, not noise, so it's coloured
  // apart from the throwaway one rather than sharing the amber "ignore me".
  const badgeClass =
    badge === "randomized-longterm"
      ? "bg-sky-500/20 text-sky-300"
      : "bg-amber-500/20 text-amber-400";

  return (
    <div className="flex items-center gap-1.5 whitespace-nowrap">
      <span className="font-mono text-xs text-slate-300">{mac}</span>
      {badge && (
        <span
          data-testid={`emitter-randomized-badge-${emitter.id}`}
          title={
            typeof cls === "string" && cls in MAC_PERSISTENCE_LABELS
              ? MAC_PERSISTENCE_LABELS[cls as MacPersistence]
              : undefined
          }
          className={`inline-block rounded px-1.5 py-0.5 text-[10px] font-medium ${badgeClass}`}
        >
          {badge}
        </span>
      )}
    </div>
  );
}

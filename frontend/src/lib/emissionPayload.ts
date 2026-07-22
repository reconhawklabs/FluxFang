// Shared helpers for rendering an emission's `payload` (a `kind`-dependent
// jsonb blob) into human-readable columns — used by the Standalone Emissions
// table and the Sensor node's cached-emission views.

/** Coerce an unknown `payload` (e.g. `CachedEmission.payload`) into a record
 * we can index defensively. Non-objects become `{}`. */
export function payloadRecord(payload: unknown): Record<string, unknown> {
  return payload !== null && typeof payload === "object"
    ? (payload as Record<string, unknown>)
    : {};
}

/** Reads a payload key defensively — `payload`'s shape depends on `kind`, so
 * any of these may be absent. Returns an em dash when missing. */
export function payloadText(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : "—";
}

/** Like `payloadText`, but tries each key in turn — the first that's a
 * string/number wins. Lets one column render either kind's payload shape
 * (e.g. wifi's `src_mac` or bluetooth's `address`) without a `kind` branch. */
export function payloadTextAny(payload: Record<string, unknown>, keys: string[]): string {
  for (const key of keys) {
    const value = payload[key];
    if (typeof value === "string" || typeof value === "number") {
      return String(value);
    }
  }
  return "—";
}

/** Format an ISO observed-at timestamp for display; falls back to the raw
 * string if it isn't a valid date. */
export function formatObservedAt(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

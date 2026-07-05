// Central TanStack Query key registry.
//
// `useLiveEvents` invalidates these keys when the WS stream reports a
// relevant change, so every later page's `useQuery` MUST key its list off
// the matching entry here (e.g. `queryKey: queryKeys.emissions` for a plain
// list, or `queryKey: [...queryKeys.emissions, filters]` for a filtered
// one) — `invalidateQueries({ queryKey })` matches by prefix, so a filtered
// key still gets invalidated when the base key is.
//
// Conventions:
// - `emissions`/`dashboard` are invalidated on every `{"type":"emission"}`
//   WS frame (a new detection can change both the emissions list and any
//   dashboard summary/counts).
// - `notifications` is invalidated on every `{"type":"notification"}` frame.
// - On `{"type":"lagged"}` (the WS receiver dropped messages, see backend
//   `ws.rs`'s `WireOutcome::Send` lagged case) `useLiveEvents` invalidates
//   *everything* (`invalidateQueries()` with no key) since it's unknown
//   which of these were affected by the dropped messages.
export const queryKeys = {
  dashboard: ['dashboard'] as const,
  dataSources: ['dataSources'] as const,
  emissions: ['emissions'] as const,
  emitters: ['emitters'] as const,
  entities: ['entities'] as const,
  zones: ['zones'] as const,
  alertMethods: ['alertMethods'] as const,
  alertRules: ['alertRules'] as const,
  notifications: ['notifications'] as const,
};

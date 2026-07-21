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
// - `catalog(kind)` (Task 9.2, `useCatalog`) is a per-`kind` key
//   (`GET /api/catalog/:kind` — field/op definitions for `RuleBuilder`/
//   `FilterBar`). It's intentionally NOT invalidated by `useLiveEvents`: the
//   catalog is server-side static configuration, not data that changes from
//   live emissions/notifications.
// - `captureDevices` (`GET /api/system/capture-devices` — enumerated wifi
//   interfaces/serial devices for the Add-Data-Source form's dropdowns) is
//   also NOT invalidated by `useLiveEvents`: it's a live *hardware* read, not
//   data driven by emissions/notifications, so the form's own Refresh
//   control (a manual `refetch()`) is how it re-runs, not the WS stream.
// - `emitterTypes(kind)` (`GET /api/emitter-types/:kind` — valid
//   `emitter_type` key/label options for an emission kind, used by the
//   Emissions "Assign to emitter" modal's Type dropdown) is, like
//   `catalog(kind)`, static server-side config — NOT invalidated by
//   `useLiveEvents`.
// - `gpsStatus` (`GET /api/gps/status`, Phase 5) is, like `captureDevices`, a
//   live *hardware* read rather than data driven by emissions/notifications
//   — NOT invalidated by `useLiveEvents`. Callers (the Dashboard GPS block,
//   `MapView`'s center-on-user) instead poll it directly via a short
//   `refetchInterval` on their own `useQuery`.
export const queryKeys = {
  dashboard: ['dashboard'] as const,
  dataSources: ['dataSources'] as const,
  captureDevices: ['captureDevices'] as const,
  emissions: ['emissions'] as const,
  emitters: ['emitters'] as const,
  entities: ['entities'] as const,
  zones: ['zones'] as const,
  alertMethods: ['alertMethods'] as const,
  alertRules: ['alertRules'] as const,
  notifications: ['notifications'] as const,
  aiAudit: ['aiAudit'] as const,
  gpsStatus: ['gpsStatus'] as const,
  sensors: ['sensors'] as const,
  coTravel: ['coTravel'] as const,
  coTravelIgnored: ['coTravelIgnored'] as const,
  catalog: (kind: string) => ['catalog', kind] as const,
  emitterTypes: (kind: string) => ['emitterTypes', kind] as const,
};

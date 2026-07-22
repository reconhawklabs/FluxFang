// Router + auth guard (Task 2.3), composed with the live-events subscriber
// (Task 9.1). `main.tsx` supplies the `QueryClientProvider`/`BrowserRouter`
// this component runs inside.
//
// Guard order, per the task brief:
//   loading      -> splash
//   needsSetup   -> Setup (forced, any path)
//   !authed      -> Login (forced, any path)
//   else         -> AppShell + nested page routes
import { useRef } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./hooks/useAuth";
import { useLiveEvents } from "./hooks/useLiveEvents";
import { useConfig } from "./hooks/useConfig";
import { api } from "./api/client";
import AppShell from "./components/AppShell";
import Setup from "./pages/Setup";
import Login from "./pages/Login";
import Dashboard from "./pages/Dashboard";
import DataSources from "./pages/DataSources";
import Sensors from "./pages/Sensors";
import Emissions from "./pages/Emissions";
import Emitters from "./pages/Emitters";
import EmitterDetailPage from "./pages/EmitterDetailPage";
import CoTravel from "./pages/CoTravel";
import Entities from "./pages/Entities";
import EntityDetailPage from "./pages/EntityDetailPage";
import Zones from "./pages/Zones";
import ZoneDetailPage from "./pages/ZoneDetailPage";
import Alerts from "./pages/Alerts";
import Notifications from "./pages/Notifications";
import MapView from "./pages/MapView";
import AiAuditLog from "./pages/AiAuditLog";
import Settings from "./pages/Settings";

export default function App() {
  const { needsSetup, authed, loading, refetch } = useAuth();
  useLiveEvents({ enabled: authed });
  const { data: config, isSuccess: configLoaded, isError: configError } = useConfig(authed);
  const isSensor = config?.role === "sensor";

  // Gate on the node config only until it FIRST settles (success or error),
  // then never again. On an upgraded install `/api/config` 404s until its
  // backfill migration runs; React Query treats that errored, data-less query
  // as perpetually stale and refetches it on every new observer mount. If we
  // kept gating "Loading" on the pending state, rendering the shell would
  // mount fresh `useConfig` observers (AppShell, Dashboard), trigger a refetch
  // (→ pending → Loading again → shell unmounts → refetch resolves → error →
  // shell remounts → …), oscillating forever and hammering the API — exactly
  // the infinite `GET /api/config` loop seen on legacy databases. Latching on
  // the first settle breaks the cycle: once we've waited once, we render (a
  // failed config falls back to the Standalone default) and stop gating on it.
  // We latch on a real settle (`isSuccess || isError`), not merely "not
  // loading", so the disabled pre-auth query doesn't latch early and flash the
  // Standalone nav before a Sensor node's config resolves post-login.
  const configSettledRef = useRef(false);
  if (configLoaded || configError) {
    configSettledRef.current = true;
  }

  if (loading || (authed && !configSettledRef.current)) {
    return (
      <div className="flex h-screen items-center justify-center bg-slate-950 text-sm text-slate-400">
        Loading…
      </div>
    );
  }

  if (needsSetup) {
    return (
      <Routes>
        <Route path="*" element={<Setup onSetupComplete={refetch} />} />
      </Routes>
    );
  }

  if (!authed) {
    return (
      <Routes>
        <Route path="*" element={<Login onLoginSuccess={refetch} />} />
      </Routes>
    );
  }

  async function handleLogout(): Promise<void> {
    await api.logout();
    await refetch();
  }

  return (
    <Routes>
      <Route path="/" element={<AppShell onLogout={handleLogout} />}>
        <Route index element={<Navigate to="/dashboard" replace />} />
        <Route path="dashboard" element={<Dashboard />} />
        <Route path="data-sources" element={<DataSources />} />
        <Route path="emissions" element={<Emissions />} />
        <Route path="notifications" element={<Notifications />} />
        <Route path="settings" element={<Settings />} />
        {!isSensor && (
          <>
            <Route path="sensors" element={<Sensors />} />
            <Route path="emitters" element={<Emitters />} />
            <Route path="emitters/:id" element={<EmitterDetailPage />} />
            <Route path="co-travel" element={<CoTravel />} />
            <Route path="entities" element={<Entities />} />
            <Route path="entities/:id" element={<EntityDetailPage />} />
            <Route path="zones" element={<Zones />} />
            <Route path="zones/:id" element={<ZoneDetailPage />} />
            <Route path="map" element={<MapView />} />
            <Route path="alerts" element={<Alerts />} />
            <Route path="ai-audit" element={<AiAuditLog />} />
          </>
        )}
        <Route path="*" element={<Navigate to="/dashboard" replace />} />
      </Route>
    </Routes>
  );
}

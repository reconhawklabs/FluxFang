// Router + auth guard (Task 2.3), composed with the live-events subscriber
// (Task 9.1). `main.tsx` supplies the `QueryClientProvider`/`BrowserRouter`
// this component runs inside.
//
// Guard order, per the task brief:
//   loading      -> splash
//   needsSetup   -> Setup (forced, any path)
//   !authed      -> Login (forced, any path)
//   else         -> AppShell + nested page routes
import { Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./hooks/useAuth";
import { useLiveEvents } from "./hooks/useLiveEvents";
import { api } from "./api/client";
import AppShell from "./components/AppShell";
import Setup from "./pages/Setup";
import Login from "./pages/Login";
import Dashboard from "./pages/Dashboard";
import DataSources from "./pages/DataSources";
import Emissions from "./pages/Emissions";
import Emitters from "./pages/Emitters";
import EmitterDetailPage from "./pages/EmitterDetailPage";
import Entities from "./pages/Entities";
import Zones from "./pages/Zones";
import Alerts from "./pages/Alerts";
import Notifications from "./pages/Notifications";
import MapView from "./pages/MapView";

export default function App() {
  const { needsSetup, authed, loading, refetch } = useAuth();
  useLiveEvents({ enabled: authed });

  if (loading) {
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
        <Route path="emitters" element={<Emitters />} />
        <Route path="emitters/:id" element={<EmitterDetailPage />} />
        <Route path="entities" element={<Entities />} />
        <Route path="zones" element={<Zones />} />
        <Route path="map" element={<MapView />} />
        <Route path="alerts" element={<Alerts />} />
        <Route path="notifications" element={<Notifications />} />
        <Route path="*" element={<Navigate to="/dashboard" replace />} />
      </Route>
    </Routes>
  );
}

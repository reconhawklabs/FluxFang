import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, expect, test, vi } from "vitest";
import App from "./App";
import { useAuth } from "./hooks/useAuth";
import { jsonResponse } from "./test-utils/fetchMocks";

vi.mock("./hooks/useAuth");
// `useLiveEvents` opens a real WebSocket in production; stub it out here so
// this router/guard test doesn't depend on jsdom's WS support.
vi.mock("./hooks/useLiveEvents", () => ({ useLiveEvents: vi.fn() }));
// `maplibre-gl` needs a real WebGL canvas jsdom doesn't have — the
// authed-route test below mounts the real `Dashboard`, which embeds
// `MapView` (Task 9.10). See `MapView.test.tsx`/`Dashboard.test.tsx` for the
// same mock.
vi.mock("maplibre-gl", () => ({
  default: {
    Map: class {
      addControl(): void {}
      on(): void {}
      remove(): void {}
      resize(): void {}
      getCanvas() {
        return { style: {} as Record<string, string> };
      }
    },
    NavigationControl: class {},
    Popup: class {
      setLngLat = () => this;
      setHTML = () => this;
      addTo = () => this;
      remove = () => this;
    },
  },
}));

const mockedUseAuth = vi.mocked(useAuth);

afterEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

function renderApp(initialPath = "/") {
  // `App` is rendered here the same way `main.tsx` mounts it in production
  // (inside a `QueryClientProvider`) — the authed route now mounts the real
  // `Dashboard` (Task 9.10), which issues `useQuery` calls that need one.
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[initialPath]}>
        <App />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("shows a loading splash while auth state is unknown", () => {
  mockedUseAuth.mockReturnValue({
    needsSetup: false,
    authed: false,
    loading: true,
    refetch: vi.fn(),
  });
  renderApp();
  expect(screen.getByText(/loading/i)).toBeInTheDocument();
});

test("routes to Setup when needs_setup is true, regardless of path", () => {
  mockedUseAuth.mockReturnValue({
    needsSetup: true,
    authed: false,
    loading: false,
    refetch: vi.fn(),
  });
  renderApp("/dashboard");
  expect(
    screen.getByRole("button", { name: /finish setup/i }),
  ).toBeInTheDocument();
});

test("routes to Login when setup is done but not authed", () => {
  mockedUseAuth.mockReturnValue({
    needsSetup: false,
    authed: false,
    loading: false,
    refetch: vi.fn(),
  });
  renderApp();
  expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument();
});

test("renders the AppShell with full nav once authed", async () => {
  mockedUseAuth.mockReturnValue({
    needsSetup: false,
    authed: true,
    loading: false,
    refetch: vi.fn(),
  });
  // The real `Dashboard` (Task 9.10) fires several `GET` queries on mount —
  // this test only cares about the shell/nav, so every route resolves to an
  // empty collection (safe for all of Dashboard's/MapView's `?? []`-guarded
  // reads). `/api/config` also falls through this catch-all; an empty array
  // isn't a valid `AppConfig`, so `role` reads as `undefined`, which is not
  // `"sensor"` — same as the default/standalone case Task 6 wires up.
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(jsonResponse([]))),
  );
  renderApp();
  // Task 6's `App` now gates the authed routes behind `useConfig(authed)`
  // settling (so a sensor never flashes the analysis nav), so the shell
  // only mounts after that fetch resolves — wait for it rather than
  // asserting synchronously right after render.
  expect(
    await screen.findByRole("link", { name: /data sources/i }),
  ).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /emitters/i })).toBeInTheDocument();
  // "/" redirects to "/dashboard", now the real Dashboard page (Task 9.10),
  // which fires its own async queries — wait for it too.
  expect(
    await screen.findByRole("heading", { name: /dashboard/i }),
  ).toBeInTheDocument();
});


test("renders the app (as Standalone) when /api/config fails, instead of looping on Loading", async () => {
  // Regression: on an upgraded install `/api/config` 404s until its backfill
  // migration runs. The config query then errors with no data and refetches
  // on every remount; gating "Loading" on it used to oscillate forever. The
  // app must fall through to the Standalone view instead.
  mockedUseAuth.mockReturnValue({
    needsSetup: false,
    authed: true,
    loading: false,
    refetch: vi.fn(),
  });
  let configCalls = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes("/api/config")) {
        configCalls += 1;
        return Promise.resolve(jsonResponse({}, 404));
      }
      // Dashboard's GPS block dereferences `status` off this payload, so it
      // needs a well-formed `GpsStatus` (an empty array would crash it — a
      // mock artifact, not a real backend shape).
      if (url.includes("/api/gps/status")) {
        return Promise.resolve(
          jsonResponse({
            source_running: false,
            has_fix: false,
            lat: null,
            lon: null,
            quality: null,
            fix_age_seconds: null,
            status: "disabled",
          }),
        );
      }
      return Promise.resolve(jsonResponse([]));
    }),
  );
  // Render at "/" exactly like the "full nav once authed" test above; the only
  // difference here is `/api/config` 404s instead of returning data. The shell
  // must still mount (with the standalone-only "Emitters" link), proving the
  // loading guard fell through to Standalone instead of looping.
  renderApp();
  expect(
    await screen.findByRole("link", { name: /emitters/i }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("link", { name: /data sources/i }),
  ).toBeInTheDocument();

  // The old bug hammered `GET /api/config` in an unbounded loop. Give any
  // residual remount/refetch churn a chance to fire, then assert the shell
  // stayed put and the request count is small (a couple of mount-triggered
  // refetches of the errored query — not the runaway loop).
  await new Promise((resolve) => setTimeout(resolve, 300));
  expect(
    screen.getByRole("link", { name: /data sources/i }),
  ).toBeInTheDocument();
  expect(configCalls).toBeLessThan(6);
});

test("renders the Sensor nav (no Standalone flash) when /api/config reports a sensor role", async () => {
  // Guards the latch: it must wait for the config to actually settle to
  // `role: "sensor"` before rendering, never briefly showing the
  // standalone-only nav. A regression that latched on "not loading" (instead
  // of a real success/error settle) would flash Emitters/Entities here.
  mockedUseAuth.mockReturnValue({
    needsSetup: false,
    authed: true,
    loading: false,
    refetch: vi.fn(),
  });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes("/api/config")) {
        return Promise.resolve(
          jsonResponse({ role: "sensor", node_sensor_id: "edge-1" }),
        );
      }
      return Promise.resolve(jsonResponse([]));
    }),
  );
  renderApp();
  // The Sensor's slim nav includes Data Sources but never the standalone-only
  // analysis links.
  expect(
    await screen.findByRole("link", { name: /data sources/i }),
  ).toBeInTheDocument();
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(
    screen.queryByRole("link", { name: /emitters/i }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("link", { name: /entities/i }),
  ).not.toBeInTheDocument();
});

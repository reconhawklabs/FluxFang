// Task 9.10 acceptance tests. `maplibre-gl` is mocked wholesale (same
// `FakeMap` shape as `MapView.test.tsx`) since Dashboard embeds `MapView`
// directly and jsdom has no real WebGL canvas.
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import Dashboard from "./Dashboard";
import { useLiveEvents } from "../hooks/useLiveEvents";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { DataSource } from "../api/dataSources";
import type { Emission, EmissionsPage } from "../api/emissions";
import type { Emitter } from "../api/emitters";
import type { Entity, EntityDetail } from "../api/entities";
import type { GpsStatus } from "../api/gps";
import type { NotificationsPage } from "../api/notifications";
import type { Zone } from "../api/zones";

// `jumpTo`/`flyTo` back the embedded `MapView`'s center-on-user-load /
// "recenter to me" button (Phase 5) — spies so a test could assert on them,
// though this file's own tests don't (see `MapView.test.tsx` for that);
// they still need to exist on the mock so `MapView` doesn't crash calling
// them from inside a `Dashboard` render.
vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(_options: unknown) {}
    addControl(): void {}
    // Accepts both the 2-arg (`on('load', cb)`) and the layer-scoped 3-arg
    // (`on('click', 'layer', cb)`) forms MapView uses; only 'load' fires here.
    on(event: string, layerOrCb: string | (() => void)): void {
      if (event === "load" && typeof layerOrCb === "function") layerOrCb();
    }
    getCanvas() {
      return { style: {} as Record<string, string> };
    }
    remove(): void {}
    resize(): void {}
    addSource(): void {}
    addLayer(): void {}
    getSource() {
      return { setData: vi.fn(), setTiles: vi.fn() };
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
    jumpTo = vi.fn();
    flyTo = vi.fn();
  }

  class FakeNavigationControl {}

  class FakePopup {
    setLngLat = vi.fn(() => this);
    setHTML = vi.fn(() => this);
    addTo = vi.fn(() => this);
    remove = vi.fn(() => this);
    constructor(_options: unknown) {}
  }

  return {
    default: {
      Map: FakeMap,
      NavigationControl: FakeNavigationControl,
      Popup: FakePopup,
    },
  };
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

/** Method+pathname-aware fetch mock (same convention as
 * `MapView.test.tsx`/`Emitters.test.tsx`'s `mockRoutes`) — each handler may
 * also inspect the query string via `url.searchParams` (used below to tell
 * the Dashboard's own small "feed" fetch apart from `MapView`'s `limit=500`
 * heatmap fetch, both of which hit `GET /api/emissions`). */
function mockRoutes(
  handlers: Record<string, (url: URL, init?: RequestInit) => unknown>,
) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const raw = typeof input === "string" ? input : input.toString();
    const url = new URL(raw, "http://localhost");
    const method = (init?.method ?? "GET").toUpperCase();
    const key = `${method} ${url.pathname}`;
    const handler = handlers[key];
    if (!handler) {
      return Promise.reject(
        new Error(`mockRoutes: no route registered for ${key}`),
      );
    }
    return Promise.resolve(jsonResponse(handler(url, init)));
  });
}

const DATA_SOURCE_RUNNING: DataSource = {
  id: "ds-1",
  created_at: "2026-01-01T00:00:00Z",
  kind: "wifi",
  mode: "monitor",
  interface: "wlan0",
  status: "running",
  config: {},
  last_error: null,
};

const DATA_SOURCE_STOPPED: DataSource = {
  ...DATA_SOURCE_RUNNING,
  id: "ds-2",
  status: "stopped",
};

const EMITTER_1: Emitter = {
  id: "emitter-1",
  name: "Kitchen AP",
  type: "wifi-ap",
  entity_id: null,
  match_criteria: { match: "all", conditions: [] },
  first_seen_at: null,
  last_seen_at: null,
  created_at: "2026-07-01T00:00:00Z",
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: "wifi-ap",
  category: null,
};

const ENTITY_1: Entity = {
  id: "entity-1",
  name: "Bob",
  notes: null,
  created_at: "2026-06-01T00:00:00Z",
};

const ENTITY_1_DETAIL: EntityDetail = {
  ...ENTITY_1,
  last_seen: "2026-07-04T12:00:00Z",
  emitters: [],
  recent_detections: [
    {
      emitter_id: null,
      lat: 1.5,
      lon: 2.5,
      observed_at: "2026-07-04T12:00:00Z",
    },
  ],
};

const ZONE_1: Zone = {
  id: "zone-1",
  name: "Home",
  lon: 2.5,
  lat: 1.5,
  radius_m: 50,
  notes: null,
  created_at: "2026-01-01T00:00:00Z",
};

const EMISSION_1: Emission = {
  id: "em-1",
  data_source_id: "ds-1",
  emitter_id: "emitter-1",
  session_id: null,
  observed_at: "2026-07-05T00:00:00Z",
  signal_strength: -40,
  lon: 2.5,
  lat: 1.5,
  kind: "wifi",
  payload: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "HomeNet", channel: 6 },
};

const EMISSION_2: Emission = {
  ...EMISSION_1,
  id: "em-2",
  emitter_id: null,
  observed_at: "2026-07-05T00:01:00Z",
  payload: { bssid: "11:22:33:44:55:66", ssid: "Other", channel: 11 },
};

const NOTIFICATIONS_PAGE: NotificationsPage = {
  items: [],
  total: 5,
  unread_count: 3,
};

const GPS_STATUS_DISABLED: GpsStatus = {
  source_running: false,
  has_fix: false,
  lat: null,
  lon: null,
  quality: null,
  fix_age_seconds: null,
  status: "disabled",
};

const GPS_STATUS_ACTIVE: GpsStatus = {
  source_running: true,
  has_fix: true,
  lat: 37.12345,
  lon: -122.6789,
  quality: 4,
  fix_age_seconds: 2.5,
  status: "active",
};

/** Distinguishes the Dashboard's own feed fetch (small `limit`) from
 * `MapView`'s heatmap fetch (`limit=500`, see that component's module doc
 * comment) since both hit `GET /api/emissions`. */
function emissionsHandler(
  feedPage: EmissionsPage,
  mapPage: EmissionsPage = feedPage,
) {
  return (url: URL) =>
    url.searchParams.get("limit") === "500" ? mapPage : feedPage;
}

function baseRoutes(
  overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {},
) {
  return {
    "GET /api/data-sources": () => [DATA_SOURCE_RUNNING, DATA_SOURCE_STOPPED],
    "GET /api/emitters": () => ({ items: [EMITTER_1], total: 1 }),
    "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
    "GET /api/entities/entity-1": () => ENTITY_1_DETAIL,
    "GET /api/zones": () => [ZONE_1],
    "GET /api/notifications": () => NOTIFICATIONS_PAGE,
    "GET /api/emissions": emissionsHandler({ items: [EMISSION_1], total: 1 }),
    "GET /api/gps/status": () => GPS_STATUS_DISABLED,
    ...overrides,
  };
}

test("KPI tiles show active data sources / emitter / entity / unread-notification counts", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<Dashboard />, { wrapper });

  // 2 data sources, only 1 `running`.
  await waitFor(() =>
    expect(
      within(screen.getByTestId("stat-tile-Active Data Sources")).getByText(
        "1",
      ),
    ).toBeInTheDocument(),
  );
  expect(
    within(screen.getByTestId("stat-tile-Emitters")).getByText("1"),
  ).toBeInTheDocument();
  expect(
    within(screen.getByTestId("stat-tile-Entities")).getByText("1"),
  ).toBeInTheDocument();
  expect(
    within(screen.getByTestId("stat-tile-Unread Notifications")).getByText("3"),
  ).toBeInTheDocument();
});

test("live emission feed renders recent emissions from the emissions query", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": emissionsHandler({
        items: [EMISSION_1, EMISSION_2],
        total: 2,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Dashboard />, { wrapper });

  await waitFor(() =>
    expect(screen.getByTestId("dashboard-feed-row-em-1")).toBeInTheDocument(),
  );
  const row1 = screen.getByTestId("dashboard-feed-row-em-1");
  // BSSID and Channel columns were removed from the feed; SSID + Emitter stay.
  expect(within(row1).queryByText("aa:bb:cc:dd:ee:ff")).not.toBeInTheDocument();
  expect(within(row1).getByText("HomeNet")).toBeInTheDocument();
  expect(within(row1).getByText("Kitchen AP")).toBeInTheDocument();

  const row2 = screen.getByTestId("dashboard-feed-row-em-2");
  expect(within(row2).queryByText("11:22:33:44:55:66")).not.toBeInTheDocument();
  expect(within(row2).getByText("—")).toBeInTheDocument(); // unassigned emitter column
});

test('the feed defaults to the "All Emissions" tab and scopes to a data source when its tab is clicked', async () => {
  const feedCalls: URL[] = [];
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": (url) => {
        if (url.searchParams.get("limit") !== "500") feedCalls.push(url);
        return { items: [EMISSION_1], total: 1 };
      },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Dashboard />, { wrapper });

  // "All Emissions" tab is present and first; the default feed fetch carries
  // no data_source_id.
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "All Emissions" }),
    ).toBeInTheDocument(),
  );
  await waitFor(() => expect(feedCalls.length).toBeGreaterThan(0));
  expect(
    feedCalls.every((url) => url.searchParams.get("data_source_id") === null),
  ).toBe(true);

  // Clicking the per-source tab re-queries the feed scoped to that source.
  // (Both mock sources share the `wlan0` interface, so the first tab is ds-1.)
  fireEvent.click(screen.getAllByRole("button", { name: "wifi (wlan0)" })[0]);
  await waitFor(() =>
    expect(
      feedCalls.some(
        (url) => url.searchParams.get("data_source_id") === "ds-1",
      ),
    ).toBe(true),
  );
});

test("dashboard time-range selector offers 15m/1h/4h/24h and defaults to 1h", async () => {
  vi.stubGlobal("fetch", mockRoutes(baseRoutes()));
  render(<Dashboard />, { wrapper });
  const select = (await screen.findByLabelText(
    /time range/i,
  )) as HTMLSelectElement;
  expect(select.value).toBe("1h");
  const labels = Array.from(select.options).map((o) => o.textContent);
  expect(labels).toEqual([
    "Past 15 min",
    "Past hour",
    "Past 4 hours",
    "Past 24 hours",
  ]);
});

test('a WS "emission" frame refreshes the feed with newly arrived emissions', async () => {
  class FakeSocket {
    onopen: (() => void) | null = null;
    onmessage: ((event: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    close = vi.fn();
  }

  let socket: FakeSocket | undefined;
  const wsFactory = vi.fn(() => {
    socket = new FakeSocket();
    return socket as unknown as WebSocket;
  });

  // First feed fetch returns just EMISSION_1; after the WS frame invalidates
  // `queryKeys.emissions`, the refetch returns EMISSION_2 prepended too.
  let feedCallCount = 0;
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": (url: URL) => {
        if (url.searchParams.get("limit") === "500")
          return { items: [EMISSION_1], total: 1 };
        feedCallCount += 1;
        return feedCallCount === 1
          ? { items: [EMISSION_1], total: 1 }
          : { items: [EMISSION_2, EMISSION_1], total: 2 };
      },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  function Harness() {
    useLiveEvents({ enabled: true, wsFactory });
    return <Dashboard />;
  }

  render(<Harness />, { wrapper });

  await waitFor(() =>
    expect(screen.getByTestId("dashboard-feed-row-em-1")).toBeInTheDocument(),
  );
  expect(
    screen.queryByTestId("dashboard-feed-row-em-2"),
  ).not.toBeInTheDocument();

  socket!.onmessage?.({
    data: JSON.stringify({ type: "emission", data: { id: "em-2" } }),
  });

  await waitFor(() =>
    expect(screen.getByTestId("dashboard-feed-row-em-2")).toBeInTheDocument(),
  );
});

test("GPS Status block shows Active + lat/lon when GET /api/gps/status has a fix", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/gps/status": () => GPS_STATUS_ACTIVE }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Dashboard />, { wrapper });

  const block = await screen.findByTestId("gps-status-block");
  await waitFor(() =>
    expect(within(block).getByText("Active")).toBeInTheDocument(),
  );
  expect(within(block).getByText(/37\.12345/)).toBeInTheDocument();
  expect(within(block).getByText(/-122\.6789/)).toBeInTheDocument();
});

test("GPS Status block shows a disabled state when GET /api/gps/status reports no source", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/gps/status": () => GPS_STATUS_DISABLED }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Dashboard />, { wrapper });

  const block = await screen.findByTestId("gps-status-block");
  await waitFor(() =>
    expect(within(block).getByText(/disabled/i)).toBeInTheDocument(),
  );
  expect(within(block).getByText("—")).toBeInTheDocument();
});

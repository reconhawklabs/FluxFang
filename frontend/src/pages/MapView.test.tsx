// Task 9.7 acceptance test, restructured for Phase 6 (Map page control
// redesign — grouped checkbox controls, datetime pickers, basemap
// switcher). Per the task brief, GL rendering itself isn't under test here
// (that's what `components/mapData.test.ts` covers) — this file only checks
// the page's non-GL surface: it renders the "Emissions"/"Layers"/"Sources"
// control groups and the basemap switcher without crashing, and asserts the
// all-vs-specific disabled-state wiring + query params those groups drive.
// `maplibre-gl` is mocked wholesale so `new maplibregl.Map(...)` never
// touches a real WebGL canvas (jsdom has none) — see `MapView.tsx`'s module
// doc comment.
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
import maplibregl from "maplibre-gl";
import MapView from "./MapView";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { Emission, EmissionsPage } from "../api/emissions";
import type { Entity, EntityDetail } from "../api/entities";
import type { Zone } from "../api/zones";
import type { DataSource } from "../api/dataSources";
import type { Emitter } from "../api/emitters";
import type { GpsStatus } from "../api/gps";

// `jumpTo` backs the once-on-load auto-center (Phase 5); `flyTo` backs the
// "recenter to me" button. Both are spies (not just no-ops) so tests below
// can assert on the `{center: [lon, lat]}` they were called with.
//
// `getSource` is source-id-aware (a `Map<string, {setData, setTiles}>`), not
// a fresh mock per call — Phase 6's basemap-switcher test needs to grab the
// SAME `setTiles` spy that `MapView`'s effect called, so a real per-id
// registry (rather than a brand-new `vi.fn()` on every `getSource(...)`
// call) is required.
vi.mock("maplibre-gl", () => {
  class FakeSource {
    setData = vi.fn();
    setTiles = vi.fn();
  }

  class FakeMap {
    // Tracks every constructed instance so tests can grab "the map MapView
    // just created" (`latestFakeMap()` below) without MapView itself
    // exposing its internal `mapRef`.
    static instances: FakeMap[] = [];
    private handlers = new Map<string, () => void>();
    // Layer-scoped handlers (`on('click', 'layer-id', cb)`), keyed
    // `${event}:${layer}` — `fireLayerEvent` below drives them in tests.
    private layerHandlers = new Map<string, (e: unknown) => void>();
    private sources = new Map<string, FakeSource>();
    jumpTo = vi.fn();
    flyTo = vi.fn();
    constructor(_options: unknown) {
      FakeMap.instances.push(this);
    }
    addControl(): void {}
    on(
      event: string,
      layerOrCb: string | (() => void),
      maybeCb?: (e: unknown) => void,
    ): void {
      if (typeof layerOrCb === "function") {
        this.handlers.set(event, layerOrCb);
        if (event === "load") layerOrCb();
      } else if (maybeCb) {
        this.layerHandlers.set(`${event}:${layerOrCb}`, maybeCb);
      }
    }
    fireLayerEvent(event: string, layer: string, e: unknown): void {
      this.layerHandlers.get(`${event}:${layer}`)?.(e);
    }
    getCanvas() {
      return { style: {} as Record<string, string> };
    }
    remove(): void {}
    resize(): void {}
    addSource(id: string): void {
      if (!this.sources.has(id)) this.sources.set(id, new FakeSource());
    }
    addLayer(): void {}
    getSource(id: string) {
      if (!this.sources.has(id)) this.sources.set(id, new FakeSource());
      return this.sources.get(id);
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
    setPaintProperty(): void {}
  }

  class FakeNavigationControl {}

  // Records constructed popups + the HTML last set on each, so the
  // emitter-marker click test can assert on the popup content.
  class FakePopup {
    static instances: FakePopup[] = [];
    html = "";
    setLngLat = vi.fn(() => this);
    setHTML = vi.fn((html: string) => {
      this.html = html;
      return this;
    });
    addTo = vi.fn(() => this);
    remove = vi.fn(() => this);
    constructor(_options: unknown) {
      FakePopup.instances.push(this);
    }
  }

  return {
    default: {
      Map: FakeMap,
      NavigationControl: FakeNavigationControl,
      Popup: FakePopup,
    },
  };
});

/** The most recently constructed `FakeMap` (there's exactly one per
 * `render(<MapView />)`) — used to assert on `jumpTo`/`flyTo`/source calls
 * without MapView exposing its internal `mapRef`. */
function latestFakeMap() {
  const MapCtor = maplibregl.Map as unknown as {
    instances: Array<{
      jumpTo: ReturnType<typeof vi.fn>;
      flyTo: ReturnType<typeof vi.fn>;
      getSource: (id: string) => {
        setData: ReturnType<typeof vi.fn>;
        setTiles: ReturnType<typeof vi.fn>;
      };
      fireLayerEvent: (event: string, layer: string, e: unknown) => void;
    }>;
  };
  return MapCtor.instances[MapCtor.instances.length - 1];
}

/** The most recently constructed `FakePopup` (MapView makes one per map) —
 * used by the emitter-marker click test to read the HTML it was filled with. */
function lastFakePopup() {
  const PopupCtor = (maplibregl as unknown as { Popup: unknown }).Popup as {
    instances: Array<{ html: string; setHTML: ReturnType<typeof vi.fn> }>;
  };
  return PopupCtor.instances[PopupCtor.instances.length - 1];
}

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

const DATA_SOURCE_1: DataSource = {
  id: "ds-1",
  created_at: "2026-01-01T00:00:00Z",
  kind: "wifi",
  mode: "monitor",
  interface: "wlan0",
  status: "running",
  config: {},
  last_error: null,
};

const EMISSION_1: Emission = {
  id: "em-1",
  data_source_id: "ds-1",
  emitter_id: null,
  session_id: null,
  observed_at: "2026-07-01T00:00:00Z",
  signal_strength: -40,
  lon: 2.5,
  lat: 1.5,
  kind: "wifi",
  payload: {},
};

const EMISSION_NO_LOCATION: Emission = {
  ...EMISSION_1,
  id: "em-2",
  lon: null,
  lat: null,
};

const EMISSIONS_PAGE: EmissionsPage = {
  items: [EMISSION_1, EMISSION_NO_LOCATION],
  total: 2,
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

/** No running gps source / no fix — the default for tests that don't care
 * about the recenter/auto-center behavior, so the "recenter to me" button
 * renders disabled and no `jumpTo` fires. */
const GPS_STATUS_NO_FIX: GpsStatus = {
  source_running: false,
  has_fix: false,
  lat: null,
  lon: null,
  quality: null,
  fix_age_seconds: null,
  status: "disabled",
};

const GPS_STATUS_FIX: GpsStatus = {
  source_running: true,
  has_fix: true,
  lat: 1.5,
  lon: 2.5,
  quality: 4,
  fix_age_seconds: 1.2,
  status: "active",
};

function baseRoutes(
  overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {},
) {
  return {
    "GET /api/data-sources": () => [DATA_SOURCE_1],
    "GET /api/emissions": () => EMISSIONS_PAGE,
    "GET /api/emitters": () => ({ items: [] as Emitter[], total: 0 }),
    "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
    "GET /api/entities/entity-1": () => ENTITY_1_DETAIL,
    "GET /api/zones": () => [ZONE_1],
    "GET /api/gps/status": () => GPS_STATUS_NO_FIX,
    ...overrides,
  };
}

test("renders the Layers/Sources control groups (no Emissions group) without crashing", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByText("Layers")).toBeInTheDocument();
  expect(screen.getByText("Sources")).toBeInTheDocument();

  // The "Emissions" category-toggle group was removed as redundant with
  // "Sources" — Sources is now the sole emission filter.
  expect(screen.queryByText("Emissions")).not.toBeInTheDocument();
  expect(screen.queryByLabelText("All Emissions")).not.toBeInTheDocument();

  expect(screen.getByLabelText("Zones")).toBeInTheDocument();
  expect(screen.getByLabelText("Entities")).toBeInTheDocument();
  expect(screen.getByLabelText("Emitters")).toBeInTheDocument();
  expect(screen.getByLabelText("All Sources")).toBeInTheDocument();
  expect(screen.getByTestId("maplibre-container")).toBeInTheDocument();

  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
});

test("hides the whole control panel when showControls=false (Dashboard embed) but still renders the map", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView showControls={false} basemap="satellite" />, { wrapper });

  expect(await screen.findByTestId("maplibre-container")).toBeInTheDocument();
  expect(screen.queryByText("Layers")).not.toBeInTheDocument();
  expect(screen.queryByText("Sources")).not.toBeInTheDocument();
  expect(screen.queryByLabelText(/basemap/i)).not.toBeInTheDocument();
});

test("renders one Sources checkbox per GET /api/data-sources entry, replacing the old dropdown", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByLabelText("All Sources")).toBeInTheDocument();
  expect(await screen.findByLabelText("wifi (wlan0)")).toBeInTheDocument();
  expect(screen.queryByLabelText(/data source/i)).not.toBeInTheDocument();
});

test("Sources group lists wifi sources but hides gps sources (gps provides location, not emissions)", async () => {
  const GPS_SOURCE: DataSource = {
    id: "gps-1",
    created_at: "2026-07-06T00:00:00Z",
    kind: "gps",
    mode: "gpsd",
    interface: null,
    status: "running",
    config: {},
    last_error: null,
  };
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/data-sources": () => [DATA_SOURCE_1, GPS_SOURCE] }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  expect(await screen.findByLabelText(/wifi \(wlan0\)/i)).toBeInTheDocument();
  expect(screen.queryByLabelText(/gps/i)).not.toBeInTheDocument();
});

test("toggling the Layers checkboxes does not crash the page", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  fireEvent.click(await screen.findByLabelText("Entities"));
  fireEvent.click(screen.getByLabelText("Zones"));
  fireEvent.click(screen.getByLabelText("Emitters"));
});

test("the Layers group toggles Zones/Entities/Emitters independently", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const zones = await screen.findByLabelText("Zones");
  const entities = screen.getByLabelText("Entities");
  const emitters = screen.getByLabelText("Emitters");

  expect(zones).toBeChecked();
  expect(entities).toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(zones);
  expect(zones).not.toBeChecked();
  expect(entities).toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(entities);
  expect(zones).not.toBeChecked();
  expect(entities).not.toBeChecked();
  expect(emitters).toBeChecked();

  fireEvent.click(emitters);
  expect(zones).not.toBeChecked();
  expect(entities).not.toBeChecked();
  expect(emitters).not.toBeChecked();
});

test('"All Sources" disables the per-source checkboxes; unchecking it and selecting one source adds data_source_id to the emissions query', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const allSources = await screen.findByLabelText("All Sources");
  const sourceCheckbox = await screen.findByLabelText("wifi (wlan0)");

  expect(allSources).toBeChecked();
  expect(sourceCheckbox).toBeDisabled();
  expect(sourceCheckbox).not.toBeChecked();

  fireEvent.click(allSources);
  expect(allSources).not.toBeChecked();
  expect(sourceCheckbox).not.toBeDisabled();

  fireEvent.click(sourceCheckbox);
  expect(sourceCheckbox).toBeChecked();

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([url]) => {
      const parsed = new URL(String(url), "http://localhost");
      return (
        parsed.pathname === "/api/emissions" &&
        parsed.searchParams.get("data_source_id") === "ds-1"
      );
    });
    expect(call).toBeDefined();
  });
});

test("the basemap switcher offers Standard/Satellite/Dark, defaulting to Satellite, and switching to Dark swaps the map tiles", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const select = await screen.findByLabelText(/basemap/i);
  expect((select as HTMLSelectElement).value).toBe("satellite");
  expect(
    within(select)
      .getAllByRole("option")
      .map((option) => option.textContent),
  ).toEqual(["Standard", "Satellite", "Dark"]);

  fireEvent.change(select, { target: { value: "dark" } });
  expect((select as HTMLSelectElement).value).toBe("dark");

  await waitFor(() => {
    expect(
      latestFakeMap().getSource("basemap-source").setTiles,
    ).toHaveBeenCalledWith(
      expect.arrayContaining([expect.stringContaining("cartocdn.com")]),
    );
  });
});

test('with no GPS fix, the "Recenter to me" button is disabled and the map never auto-centers', async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole("button", { name: /recenter/i });
  await waitFor(() => expect(button).toBeDisabled());
  expect(latestFakeMap().jumpTo).not.toHaveBeenCalled();
});

test('with a GPS fix, the map auto-centers on load and the "Recenter to me" button is enabled', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/gps/status": () => GPS_STATUS_FIX }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole("button", { name: /recenter/i });
  await waitFor(() => expect(button).toBeEnabled());
  await waitFor(() =>
    expect(latestFakeMap().jumpTo).toHaveBeenCalledWith(
      expect.objectContaining({
        center: [GPS_STATUS_FIX.lon, GPS_STATUS_FIX.lat],
      }),
    ),
  );
});

test('clicking "Recenter to me" flies the map to the current GPS fix', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/gps/status": () => GPS_STATUS_FIX }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });

  const button = await screen.findByRole("button", { name: /recenter/i });
  await waitFor(() => expect(button).toBeEnabled());

  fireEvent.click(button);

  expect(latestFakeMap().flyTo).toHaveBeenCalledWith(
    expect.objectContaining({
      center: [GPS_STATUS_FIX.lon, GPS_STATUS_FIX.lat],
    }),
  );
});

test("Auto Track recenters on GPS immediately and every 5s while enabled", async () => {
  vi.useFakeTimers();
  try {
    const fetchMock = mockRoutes(
      baseRoutes({ "GET /api/gps/status": () => GPS_STATUS_FIX }),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(<MapView />, { wrapper });
    // let the map 'load' + first GPS poll resolve
    await vi.advanceTimersByTimeAsync(0);
    const map = latestFakeMap();

    const button = screen.getByRole("button", { name: /auto track/i });
    expect(button).toBeEnabled();
    const before = map.flyTo.mock.calls.length;

    fireEvent.click(button);
    // immediate recenter on enable
    expect(map.flyTo.mock.calls.length - before).toBe(1);

    await vi.advanceTimersByTimeAsync(5000);
    expect(map.flyTo.mock.calls.length - before).toBe(2);

    fireEvent.click(button); // disable
    await vi.advanceTimersByTimeAsync(5000);
    expect(map.flyTo.mock.calls.length - before).toBe(2); // no more calls
  } finally {
    vi.useRealTimers();
  }
});

test("clicking an emitter marker opens a popup with its details", async () => {
  const emitter: Emitter = {
    id: "emitter-1",
    name: "Kitchen AP",
    type: null,
    emitter_type: "wifi_access_point",
    attributes: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "HomeNet" },
    match_enabled: true,
    type_label: "WiFi Access Point",
    category: "wifi",
    entity_id: null,
    match_criteria: { match: "all", conditions: [] },
    first_seen_at: "2026-07-05T00:00:00Z",
    last_seen_at: "2026-07-05T01:00:00Z",
    created_at: "2026-07-05T00:00:00Z",
  };
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/emitters": () => ({ items: [emitter], total: 1 }) }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<MapView />, { wrapper });
  await screen.findByTestId("maplibre-container");

  const feature = {
    geometry: { type: "Point", coordinates: [2.5, 1.5] },
    properties: {
      id: "emitter-1",
      name: "Kitchen AP",
      observed_at: "2026-07-05T01:00:00Z",
    },
  };

  // Retry the click until the emitters query has populated the id→emitter
  // lookup the popup reads from (the marker layer is clickable immediately,
  // but the rich fields only appear once `GET /api/emitters` resolves).
  await waitFor(() => {
    latestFakeMap().fireLayerEvent("click", "emitter-circle-layer", {
      features: [feature],
    });
    const popup = lastFakePopup();
    expect(popup.html).toContain("Kitchen AP");
    expect(popup.html).toContain("WiFi Access Point");
    expect(popup.html).toContain("aa:bb:cc:dd:ee:ff");
  });
});

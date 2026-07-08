import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  render,
  screen,
  waitFor,
  fireEvent,
  within,
} from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, expect, test, vi } from "vitest";
import EmitterDetailPage from "./EmitterDetailPage";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { Emitter } from "../api/emitters";

// EmissionsHeatmap inits a real MapLibre map for non-empty points — mock it.
vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl(): void {}
    on(event: string, cb: () => void): void {
      if (event === "load") cb();
    }
    remove(): void {}
    resize(): void {}
    addSource(): void {}
    addLayer(): void {}
    getSource() {
      return { setData: vi.fn() };
    }
    getLayer() {
      return true;
    }
    setLayoutProperty(): void {}
    fitBounds(): void {}
  }
  return { default: { Map: FakeMap, NavigationControl: class {} } };
});

afterEach(() => vi.unstubAllGlobals());

const EMITTER: Emitter = {
  id: "emitter-1",
  name: "Kitchen AP",
  type: "WiFi Access Point",
  type_label: "WiFi Access Point",
  emitter_type: "wifi_access_point",
  entity_id: null,
  match_enabled: true,
  match_criteria: {
    match: "all",
    conditions: [{ field: "bssid", op: "eq", value: "aa:bb:cc:dd:ee:ff" }],
  },
  attributes: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "KitchenNet" },
  first_seen_at: "2026-07-06T00:00:00Z",
  last_seen_at: "2026-07-06T01:00:00Z",
} as unknown as Emitter;

const BLUETOOTH_EMITTER: Emitter = {
  id: "bt-1",
  name: "Someone's Phone",
  type: "Bluetooth Device",
  type_label: "Bluetooth Device",
  emitter_type: "bluetooth_device",
  entity_id: null,
  match_enabled: true,
  match_criteria: { match: "all", conditions: [] },
  attributes: {
    address: "3c:15:c2:aa:bb:cc",
    vendor: "Apple, Inc.",
    device_type: "Phone",
    randomized_mac: false,
  },
  first_seen_at: "2026-07-06T00:00:00Z",
  last_seen_at: "2026-07-06T01:00:00Z",
} as unknown as Emitter;

const CLIENT_EMITTER: Emitter = {
  id: "client-1",
  name: "Phone",
  type: "WiFi Client",
  type_label: "WiFi Client",
  emitter_type: "wifi_client",
  entity_id: null,
  match_enabled: true,
  match_criteria: {
    match: "all",
    conditions: [{ field: "src_mac", op: "eq", value: "3a:de:ad:be:ef:00" }],
  },
  attributes: { src_mac: "3a:de:ad:be:ef:00", randomized_mac: true },
  first_seen_at: "2026-07-06T00:00:00Z",
  last_seen_at: "2026-07-06T02:05:00Z",
} as unknown as Emitter;

const TPMS_EMITTER: Emitter = {
  id: "emitter-1",
  name: "Front Left Tire",
  type: "TPMS Sensor",
  type_label: "TPMS Sensor",
  emitter_type: "tpms_sensor",
  entity_id: null,
  match_enabled: true,
  match_criteria: { match: "all", conditions: [] },
  attributes: { sensor_id: "abc123" },
  first_seen_at: "2026-07-06T00:00:00Z",
  last_seen_at: "2026-07-06T01:00:00Z",
} as unknown as Emitter;

const ASSOCIATED_TPMS_EMITTER: Emitter = {
  id: "tire-2",
  name: "Rear Right Tire",
  type: "TPMS Sensor",
  type_label: "TPMS Sensor",
  emitter_type: "tpms_sensor",
  entity_id: null,
  match_enabled: true,
  match_criteria: { match: "all", conditions: [] },
  attributes: { sensor_id: "def456" },
  first_seen_at: "2026-07-06T00:00:00Z",
  last_seen_at: "2026-07-06T01:00:00Z",
} as unknown as Emitter;

function mockRoutes(
  handlers: Record<string, (url: URL, init?: RequestInit) => unknown>,
) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const raw = typeof input === "string" ? input : input.toString();
    const url = new URL(raw, "http://localhost");
    const key = `${(init?.method ?? "GET").toUpperCase()} ${url.pathname}`;
    const handler = handlers[key];
    if (!handler) return Promise.reject(new Error(`no route for ${key}`));
    return Promise.resolve(jsonResponse(handler(url, init)));
  });
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={["/emitters/emitter-1"]}>
        <Routes>
          <Route path="/emitters/:id" element={children} />
          <Route path="/emitters" element={<div>Emitters list</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<EmitterDetailPage />, { wrapper: Wrapper });
}

test("renders the emitter's name, identity and last-known coords", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({
        items: [
          {
            id: "e1",
            observed_at: "2026-07-06T01:00:00Z",
            kind: "wifi",
            payload: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "KitchenNet" },
            signal_strength: -40,
            lon: -98.5,
            lat: 39.5,
            emitter_id: "emitter-1",
          },
        ],
        total: 1,
      }),
    }),
  );
  renderPage();
  expect(
    await screen.findByRole("heading", { name: /kitchen ap/i }),
  ).toBeInTheDocument();
  const summary = screen.getByText("Identity").closest("dl");
  expect(summary).not.toBeNull();
  expect(
    within(summary as HTMLElement).getByText("aa:bb:cc:dd:ee:ff"),
  ).toBeInTheDocument();
  expect(screen.getByText(/39\.5/)).toBeInTheDocument(); // last-known lat
});

test("shows not-found on a 404", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const raw = typeof input === "string" ? input : input.toString();
      const url = new URL(raw, "http://localhost");
      if (url.pathname === "/api/emitters/emitter-1") {
        return Promise.resolve(jsonResponse({ message: "not found" }, 404));
      }
      if (url.pathname === "/api/entities") {
        return Promise.resolve(jsonResponse({ items: [], total: 0 }));
      }
      if (url.pathname === "/api/emissions") {
        return Promise.resolve(jsonResponse({ items: [], total: 0 }));
      }
      return Promise.reject(new Error(`no route for ${url.pathname}`));
    }),
  );
  renderPage();
  expect(await screen.findByText(/emitter not found/i)).toBeInTheDocument();
});

test("saving a rule POSTs to /rule", async () => {
  const fetchMock = mockRoutes({
    "GET /api/emitters/emitter-1": () => EMITTER,
    "GET /api/entities": () => ({ items: [], total: 0 }),
    "GET /api/emissions": () => ({ items: [], total: 0 }),
    "POST /api/emitters/emitter-1/rule": () => ({
      emitter: EMITTER,
      attached_count: 3,
    }),
  });
  vi.stubGlobal("fetch", fetchMock);
  renderPage();
  fireEvent.click(await screen.findByRole("button", { name: /save rule/i }));
  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/api/emitters/emitter-1/rule"),
      expect.objectContaining({ method: "POST" }),
    ),
  );
});

test("wifi client shows connected APs, linking only when the AP emitter exists", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => CLIENT_EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({
        items: [
          {
            id: "a1",
            observed_at: "2026-07-06T02:00:00Z",
            kind: "wifi",
            payload: {
              frame_type: "association_request",
              src_mac: "3a:de:ad:be:ef:00",
              target_bssid: "aa:bb:cc:dd:ee:ff",
              target_ssid: "HomeNet",
            },
            signal_strength: -50,
            lon: null,
            lat: null,
            emitter_id: "client-1",
          },
          {
            id: "a2",
            observed_at: "2026-07-06T02:05:00Z",
            kind: "wifi",
            payload: {
              frame_type: "reassociation_request",
              src_mac: "3a:de:ad:be:ef:00",
              target_bssid: "11:22:33:44:55:66",
              target_ssid: "CoffeeShop",
            },
            signal_strength: -70,
            lon: null,
            lat: null,
            emitter_id: "client-1",
          },
        ],
        total: 2,
      }),
      "GET /api/emitters": () => ({
        items: [
          {
            id: "ap-1",
            name: "Home",
            type: null,
            type_label: "WiFi Access Point",
            emitter_type: "wifi_access_point",
            entity_id: null,
            match_enabled: true,
            match_criteria: {},
            attributes: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "HomeNet" },
            first_seen_at: null,
            last_seen_at: null,
          },
        ],
        total: 1,
      }),
    }),
  );
  renderPage();

  expect(
    await screen.findByText(/connected access points/i),
  ).toBeInTheDocument();

  // The HomeNet AP exists → its BSSID links to that AP emitter.
  const homeLink = await screen.findByRole("link", {
    name: "aa:bb:cc:dd:ee:ff",
  });
  expect(homeLink).toHaveAttribute("href", "/emitters/ap-1");

  // The CoffeeShop AP has no emitter → BSSID is plain text, not a link.
  expect(screen.getByText("11:22:33:44:55:66")).toBeInTheDocument();
  expect(
    screen.queryByRole("link", { name: "11:22:33:44:55:66" }),
  ).not.toBeInTheDocument();
});

test("shows vendor and device type for a bluetooth emitter", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => BLUETOOTH_EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({ items: [], total: 0 }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", { name: /someone's phone/i }),
  ).toBeInTheDocument();
  // Scope to the summary <dl> — the Attributes section below also dumps
  // these raw key/value pairs (lowercase keys), so unscoped queries would
  // match twice.
  const summary = screen.getByText("Identity").closest("dl");
  expect(summary).not.toBeNull();
  const withinSummary = within(summary as HTMLElement);
  expect(withinSummary.getByText("Vendor")).toBeInTheDocument();
  expect(withinSummary.getByText("Apple, Inc.")).toBeInTheDocument();
  expect(withinSummary.getByText("Device type")).toBeInTheDocument();
  expect(withinSummary.getByText("Phone")).toBeInTheDocument();
  // Identity cell falls back to the bluetooth `address` attribute.
  expect(withinSummary.getByText("3c:15:c2:aa:bb:cc")).toBeInTheDocument();
});

test("omits vendor/device type lines when absent", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({ items: [], total: 0 }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", { name: /kitchen ap/i }),
  ).toBeInTheDocument();
  expect(screen.queryByText("Vendor")).not.toBeInTheDocument();
  expect(screen.queryByText("Device type")).not.toBeInTheDocument();
});

test("tpms_sensor emitter shows the latest TPMS reading, derived from the most recent emission", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => TPMS_EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({
        items: [
          {
            id: "t2",
            observed_at: "2026-07-06T02:00:00Z",
            kind: "tpms",
            payload: { status: 1, pressure_PSI: 32.5, rssi: -60, snr: 12 },
            signal_strength: -60,
            lon: null,
            lat: null,
            emitter_id: "emitter-1",
          },
          {
            id: "t1",
            observed_at: "2026-07-06T01:00:00Z",
            kind: "tpms",
            payload: { status: 0, pressure_PSI: 30.0, rssi: -70, snr: 8 },
            signal_strength: -70,
            lon: null,
            lat: null,
            emitter_id: "emitter-1",
          },
        ],
        total: 2,
      }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", { name: /latest tpms reading/i }),
  ).toBeInTheDocument();
  // Picks the most-recent emission (t2) by observed_at, not just index 0.
  expect(screen.getByText("32.5")).toBeInTheDocument();
});

test("tpms_sensor emitter shows other associated tires with a source badge and link", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => TPMS_EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({ items: [], total: 0 }),
      "GET /api/emitters/emitter-1/associations": () => [
        { emitter: ASSOCIATED_TPMS_EMITTER, source: "auto", confidence: 0.87 },
      ],
      "GET /api/emitters": () => ({
        items: [TPMS_EMITTER, ASSOCIATED_TPMS_EMITTER],
        total: 2,
      }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", {
      name: /other tires on the same car/i,
    }),
  ).toBeInTheDocument();
  const link = await screen.findByRole("link", { name: /rear right tire/i });
  expect(link).toHaveAttribute("href", "/emitters/tire-2");
  expect(screen.getByText("auto 87%")).toBeInTheDocument();
});

test("does not show the associated-tires section for a non-tpms emitter", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({ items: [], total: 0 }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", { name: /kitchen ap/i }),
  ).toBeInTheDocument();
  expect(
    screen.queryByText(/other tires on the same car/i),
  ).not.toBeInTheDocument();
});

test("does not show a Latest TPMS reading section for a non-tpms emitter", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/emitters/emitter-1": () => EMITTER,
      "GET /api/entities": () => ({ items: [], total: 0 }),
      "GET /api/emissions": () => ({
        items: [
          {
            id: "e1",
            observed_at: "2026-07-06T01:00:00Z",
            kind: "wifi",
            payload: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "KitchenNet" },
            signal_strength: -40,
            lon: -98.5,
            lat: 39.5,
            emitter_id: "emitter-1",
          },
        ],
        total: 1,
      }),
    }),
  );
  renderPage();

  expect(
    await screen.findByRole("heading", { name: /kitchen ap/i }),
  ).toBeInTheDocument();
  expect(
    screen.queryByText(/latest tpms reading/i),
  ).not.toBeInTheDocument();
});

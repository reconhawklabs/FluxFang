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
import Emitters from "./Emitters";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { Emitter } from "../api/emitters";
import type { Entity } from "../api/entities";
import type { Emission } from "../api/emissions";

// `EmitterDetail` embeds `EmissionsHeatmap` (Task C), which inits a real
// MapLibre map whenever it's given non-empty points — mocked wholesale here
// (same convention as `MapView.test.tsx`) so that never touches a real
// WebGL canvas jsdom doesn't have.
vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(_options: unknown) {}
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

  class FakeNavigationControl {}

  return {
    default: { Map: FakeMap, NavigationControl: FakeNavigationControl },
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
 * `Emissions.test.tsx`'s `mockRoutes`) — this page hits `GET /api/emitters`,
 * `GET /api/entities`, `POST /api/entities`, `PATCH /api/emitters/:id`,
 * `POST /api/emitters/bulk-delete`, `POST /api/emitters/clear`, and
 * `GET /api/emissions` (the expanded detail), so routing needs both the
 * method and the (sometimes dynamic-id) pathname. Query params are exposed
 * to the handler via `url.searchParams` so search/entity-filter/pagination
 * tests can assert on them directly. */
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

/** Default empty responses for the endpoints every render touches even when
 * a test doesn't care about them (entities list, expanded-detail
 * emissions), so each test only needs to override what it's actually
 * exercising. */
function baseRoutes(
  overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {},
) {
  return {
    "GET /api/entities": () => ({ items: [], total: 0 }),
    "GET /api/emissions": () => ({ items: [], total: 0 }),
    // The expanded-row rule editor (`RuleBuilder`) fetches the "wifi" field
    // catalog to drive its field/operator dropdowns.
    "GET /api/catalog/wifi": () => WIFI_CATALOG,
    ...overrides,
  };
}

/** Minimal `GET /api/catalog/wifi` stub — enough fields for the expanded
 * row's `RuleBuilder` to render its condition rows. */
const WIFI_CATALOG = [
  {
    key: "bssid",
    label: "BSSID",
    type: "mac",
    ops: [
      { code: "eq", label: "equals" },
      { code: "neq", label: "not equals" },
    ],
  },
  {
    key: "src_mac",
    label: "Source MAC",
    type: "mac",
    ops: [
      { code: "eq", label: "equals" },
      { code: "neq", label: "not equals" },
    ],
  },
];

const EMITTER_UNASSIGNED: Emitter = {
  id: "emitter-1",
  name: "Unknown AP",
  type: "wifi-ap",
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: null,
  category: null,
  entity_id: null,
  match_criteria: {
    match: "all",
    conditions: [{ field: "bssid", op: "eq", value: "aa:bb:cc:dd:ee:ff" }],
  },
  first_seen_at: "2026-07-01T00:00:00Z",
  last_seen_at: "2026-07-04T12:00:00Z",
  created_at: "2026-07-01T00:00:00Z",
};

const EMITTER_ASSIGNED: Emitter = {
  id: "emitter-2",
  name: "Neighbor's Router",
  type: "wifi-ap",
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: null,
  category: null,
  entity_id: "entity-1",
  match_criteria: { match: "all", conditions: [] },
  first_seen_at: null,
  last_seen_at: null,
  created_at: "2026-07-01T00:00:00Z",
};

/** An auto-classified WiFi client emitter (Phase A backend / Phase B
 * frontend, emitter auto-classification design doc) — has a randomized
 * source MAC flagged, and its rule is currently enabled. */
const EMITTER_CLIENT: Emitter = {
  id: "emitter-3",
  name: "WiFi Client aa:bb:cc:dd:ee:ff",
  type: null,
  emitter_type: "wifi_client",
  attributes: { src_mac: "aa:bb:cc:dd:ee:ff", randomized_mac: true },
  match_enabled: true,
  type_label: "WiFi Client",
  category: "wifi",
  entity_id: null,
  match_criteria: {
    match: "all",
    conditions: [{ field: "src_mac", op: "eq", value: "aa:bb:cc:dd:ee:ff" }],
  },
  first_seen_at: "2026-07-05T00:00:00Z",
  last_seen_at: "2026-07-05T01:00:00Z",
  created_at: "2026-07-05T00:00:00Z",
};

/** An auto-classified WiFi access-point emitter with a visible SSID and no
 * randomization (an AP's BSSID doesn't rotate, unlike a probing client's). */
const EMITTER_AP: Emitter = {
  id: "emitter-4",
  name: 'WiFi AP "CoffeeShop" (11:22:33:44:55:66)',
  type: null,
  emitter_type: "wifi_access_point",
  attributes: { ssid: "CoffeeShop", bssid: "11:22:33:44:55:66" },
  match_enabled: false,
  type_label: "WiFi Access Point",
  category: "wifi",
  entity_id: null,
  match_criteria: {
    match: "all",
    conditions: [{ field: "bssid", op: "eq", value: "11:22:33:44:55:66" }],
  },
  first_seen_at: "2026-07-05T00:00:00Z",
  last_seen_at: "2026-07-05T01:00:00Z",
  created_at: "2026-07-05T00:00:00Z",
};

const ENTITY_1: Entity = {
  id: "entity-1",
  name: "Bob",
  notes: null,
  created_at: "2026-06-01T00:00:00Z",
};

// --- Phase 3: compact one-row rows ---

test("a randomized-MAC client emitter renders name/type/MAC/randomized-badge all within a single one-line row", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-3");
  // The row is a single <tr> — every one of these lives inside it, proving
  // the row didn't fall back to a multi-line/stacked layout.
  expect(
    within(row).getByText("WiFi Client aa:bb:cc:dd:ee:ff"),
  ).toBeInTheDocument();
  expect(within(row).getByText("WiFi Client")).toBeInTheDocument(); // type badge, one line
  const mac = within(row).getByText("aa:bb:cc:dd:ee:ff");
  expect(mac).toHaveClass("font-mono");
  expect(
    within(row).getByTestId("emitter-randomized-badge-emitter-3"),
  ).toHaveTextContent(/randomized/i);

  // MAC and its badge share one cell (the "MAC/Identity" column) rather
  // than being stacked across separate rows/lines.
  expect(mac.closest("td")).toContainElement(
    screen.getByTestId("emitter-randomized-badge-emitter-3"),
  );

  // Only a single <tr> renders for this emitter while collapsed — no
  // second "detail" row is present.
  expect(
    screen.queryByTestId("emitter-detail-emitter-3"),
  ).not.toBeInTheDocument();
});

test("a plain AP emitter (no randomized flag) shows its BSSID monospace with no randomized badge", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_AP], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-4")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-4");
  expect(within(row).getByText("11:22:33:44:55:66")).toHaveClass("font-mono");
  expect(
    within(row).queryByTestId("emitter-randomized-badge-emitter-4"),
  ).not.toBeInTheDocument();
});

test('renders emitter rows resolving the associated entity name, and "—" when unassigned', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_UNASSIGNED, EMITTER_ASSIGNED],
        total: 2,
      }),
      "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  expect(screen.getByTestId("emitter-entity-emitter-1")).toHaveTextContent("—");
  expect(screen.getByTestId("emitter-entity-emitter-2")).toHaveTextContent(
    "Bob",
  );
});

// --- Phase 3: expand reveals rule + attributes ---

test("the match rule and attributes are only shown once the row is expanded", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );

  // Collapsed: no rule text, no attributes dump, no rule-enabled switch
  // anywhere on the page.
  expect(
    screen.queryByText(/src_mac eq aa:bb:cc:dd:ee:ff/i),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("switch", { name: /rule enabled/i }),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("src_mac")).not.toBeInTheDocument();

  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );

  const detail = await screen.findByTestId("emitter-detail-emitter-3");
  expect(
    within(detail).getByText(/src_mac eq aa:bb:cc:dd:ee:ff/i),
  ).toBeInTheDocument();
  expect(
    within(detail).getByRole("switch", { name: /rule enabled/i }),
  ).toBeChecked();
  // Full attributes dump.
  expect(within(detail).getByText("src_mac")).toBeInTheDocument();
  expect(within(detail).getByText("randomized_mac")).toBeInTheDocument();
});

test("toggling the rule switch (in the expanded panel) PATCHes {match_enabled: false}", async () => {
  const disabled: Emitter = { ...EMITTER_CLIENT, match_enabled: false };
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
      "PATCH /api/emitters/emitter-3": () => disabled,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );
  const detail = await screen.findByTestId("emitter-detail-emitter-3");

  fireEvent.click(
    within(detail).getByRole("switch", { name: /rule enabled/i }),
  );

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-3",
      expect.objectContaining({ method: "PATCH" }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-3" && init?.method === "PATCH",
  );
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ match_enabled: false });
});

test("manual randomized override (in the expanded panel) PATCHes the full attributes object with randomized_mac flipped", async () => {
  const flipped: Emitter = {
    ...EMITTER_CLIENT,
    attributes: { ...EMITTER_CLIENT.attributes, randomized_mac: false },
  };
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
      "PATCH /api/emitters/emitter-3": () => flipped,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );
  const detail = await screen.findByTestId("emitter-detail-emitter-3");

  fireEvent.click(
    within(detail).getByRole("button", { name: /mark as not randomized/i }),
  );

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-3",
      expect.objectContaining({ method: "PATCH" }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-3" && init?.method === "PATCH",
  );
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({
    attributes: { src_mac: "aa:bb:cc:dd:ee:ff", randomized_mac: false },
  });
});

test('editing the rule and clicking "Save rule" POSTs /api/emitters/:id/rule with the match_criteria', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
      "POST /api/emitters/emitter-3/rule": () => ({
        emitter: EMITTER_CLIENT,
        attached_count: 2,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );
  const detail = await screen.findByTestId("emitter-detail-emitter-3");

  // Rule editor renders seeded from the emitter's current rule.
  fireEvent.click(
    await within(detail).findByRole("button", { name: /save rule/i }),
  );

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-3/rule",
      expect.objectContaining({ method: "POST" }),
    ),
  );
  const ruleCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-3/rule" && init?.method === "POST",
  );
  const [, init] = ruleCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({
    match_criteria: {
      match: "all",
      conditions: [{ field: "src_mac", op: "eq", value: "aa:bb:cc:dd:ee:ff" }],
    },
  });

  // Success feedback reflects the backend's re-attach count.
  expect(
    await within(detail).findByText(/attached 2 emissions/i),
  ).toBeInTheDocument();
});

const LOCATED_EMISSION: Emission = {
  id: "em-1",
  data_source_id: "ds-1",
  emitter_id: "emitter-3",
  session_id: null,
  observed_at: "2026-07-05T00:00:00Z",
  signal_strength: -40,
  lon: 2.5,
  lat: 1.5,
  kind: "wifi",
  payload: {},
};

const UNLOCATED_EMISSION: Emission = {
  ...LOCATED_EMISSION,
  id: "em-2",
  lon: null,
  lat: null,
};

test("expanded detail renders a detection heatmap fed by located emissions for that emitter", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
      "GET /api/emissions": () => ({
        items: [LOCATED_EMISSION, UNLOCATED_EMISSION],
        total: 2,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );

  const detail = await screen.findByTestId("emitter-detail-emitter-3");
  expect(within(detail).getByText("Detection heatmap")).toBeInTheDocument();
  expect(
    await within(detail).findByTestId("emissions-heatmap-container"),
  ).toBeInTheDocument();
});

test("expanded detail shows the heatmap empty state when the emitter has no located emissions", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "WiFi Client aa:bb:cc:dd:ee:ff" }),
  );

  const detail = await screen.findByTestId("emitter-detail-emitter-3");
  expect(
    await within(detail).findByText("No located detections yet."),
  ).toBeInTheDocument();
});

// --- Phase 3: Associate control folds in "+ New entity…" ---

test('the Associate control has no separate "New Entity" button anywhere on the page', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  expect(
    screen.queryByRole("button", { name: /^new entity$/i }),
  ).not.toBeInTheDocument();
  const row = screen.getByTestId("emitter-row-emitter-1");
  const associateSelect = within(row).getByLabelText(
    /associate .* to an entity/i,
  );
  expect(
    within(associateSelect).getByText("+ New entity…"),
  ).toBeInTheDocument();
});

test('"+ New entity…" prompts for a name, then POSTs /api/entities and PATCHes the emitter with the new entity_id', async () => {
  const NEW_ENTITY: Entity = {
    id: "entity-new",
    name: "Coffee Shop",
    notes: null,
    created_at: "2026-07-05T00:00:00Z",
  };
  const associated: Emitter = {
    ...EMITTER_UNASSIGNED,
    entity_id: "entity-new",
  };
  vi.spyOn(window, "prompt").mockReturnValue("Coffee Shop");

  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
      "POST /api/entities": () => NEW_ENTITY,
      "PATCH /api/emitters/emitter-1": () => associated,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-1");
  fireEvent.change(within(row).getByLabelText(/associate .* to an entity/i), {
    target: { value: "__new_entity__" },
  });

  expect(window.prompt).toHaveBeenCalled();

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/entities",
      expect.objectContaining({ method: "POST" }),
    ),
  );
  const postCall = fetchMock.mock.calls.find(
    ([url, init]) => String(url) === "/api/entities" && init?.method === "POST",
  );
  const [, postInit] = postCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(postInit.body as string)).toEqual({ name: "Coffee Shop" });

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-1",
      expect.objectContaining({ method: "PATCH" }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-1" && init?.method === "PATCH",
  );
  const [, patchInit] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(patchInit.body as string)).toEqual({
    entity_id: "entity-new",
  });
});

test('declining the "+ New entity…" prompt (empty name) does not call the entities endpoint', async () => {
  vi.spyOn(window, "prompt").mockReturnValue(null);
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-1");
  fireEvent.change(within(row).getByLabelText(/associate .* to an entity/i), {
    target: { value: "__new_entity__" },
  });

  expect(window.prompt).toHaveBeenCalled();
  expect(
    fetchMock.mock.calls.find(([url]) => String(url) === "/api/entities"),
  ).toBeUndefined();
});

test("associate-to-existing: selecting an entity PATCHes {entity_id: <selected>}", async () => {
  const patched: Emitter = { ...EMITTER_UNASSIGNED, entity_id: "entity-1" };
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
      "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
      "PATCH /api/emitters/emitter-1": () => patched,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-1");
  fireEvent.change(within(row).getByLabelText(/associate .* to an entity/i), {
    target: { value: "entity-1" },
  });

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-1",
      expect.objectContaining({ method: "PATCH" }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-1" && init?.method === "PATCH",
  );
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ entity_id: "entity-1" });
});

test("detach: selecting Detach PATCHes {entity_id: null}, and Detach only appears when already associated", async () => {
  const detached: Emitter = { ...EMITTER_ASSIGNED, entity_id: null };
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_UNASSIGNED, EMITTER_ASSIGNED],
        total: 2,
      }),
      "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
      "PATCH /api/emitters/emitter-2": () => detached,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-2")).toBeInTheDocument(),
  );

  const unassignedRow = screen.getByTestId("emitter-row-emitter-1");
  expect(
    within(
      unassignedRow.querySelector("select") as HTMLSelectElement,
    ).queryByText("Detach"),
  ).toBeNull();

  const row = screen.getByTestId("emitter-row-emitter-2");
  fireEvent.change(within(row).getByLabelText(/associate .* to an entity/i), {
    target: { value: "__detach__" },
  });

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-2",
      expect.objectContaining({ method: "PATCH" }),
    ),
  );
  const patchCall = fetchMock.mock.calls.find(
    ([url, init]) =>
      String(url) === "/api/emitters/emitter-2" && init?.method === "PATCH",
  );
  const [, init] = patchCall as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ entity_id: null });
});

// --- Phase 3: search + entity filter + pagination ---

test("typing in the search bar refetches with the search param", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  fireEvent.change(screen.getByPlaceholderText("Search emitters…"), {
    target: { value: "coffee" },
  });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.get("search") === "coffee"
      );
    });
    expect(call).toBeDefined();
  });
});

test("choosing an entity in the filter refetches with the entity_id param", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_ASSIGNED], total: 1 }),
      "GET /api/entities": () => ({ items: [ENTITY_1], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-2")).toBeInTheDocument(),
  );

  fireEvent.change(screen.getByLabelText("Filter by entity"), {
    target: { value: "entity-1" },
  });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.get("entity_id") === "entity-1"
      );
    });
    expect(call).toBeDefined();
  });
});

test('the Type filter offers "All types" plus the distinct emitter types in the result set', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_CLIENT, EMITTER_AP],
        total: 2,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );

  const typeSelect = screen.getByLabelText(
    "Filter by type",
  ) as HTMLSelectElement;
  expect(
    within(typeSelect).getByRole("option", { name: "All types" }),
  ).toBeInTheDocument();
  // Derived from the two loaded emitters' distinct type labels.
  expect(
    within(typeSelect).getByRole("option", { name: "WiFi Client" }),
  ).toHaveValue("wifi_client");
  expect(
    within(typeSelect).getByRole("option", { name: "WiFi Access Point" }),
  ).toHaveValue("wifi_access_point");
});

test("choosing a type in the Type filter refetches with the emitter_type param", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_CLIENT, EMITTER_AP],
        total: 2,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-3")).toBeInTheDocument(),
  );

  fireEvent.change(screen.getByLabelText("Filter by type"), {
    target: { value: "wifi_access_point" },
  });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.get("emitter_type") === "wifi_access_point"
      );
    });
    expect(call).toBeDefined();
  });
});

test("pagination (Next) refetches with the next offset and clears the row selection", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": (url) =>
        url.searchParams.get("offset") === "50"
          ? { items: [EMITTER_ASSIGNED], total: 60 }
          : { items: [EMITTER_UNASSIGNED], total: 60 },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emitter emitter-1"));
  expect(
    screen.getByRole("button", { name: /delete selected \(1\)/i }),
  ).toBeEnabled();

  fireEvent.click(screen.getByRole("button", { name: /^next$/i }));
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-2")).toBeInTheDocument(),
  );

  expect(
    screen.getByRole("button", { name: /delete selected \(0\)/i }),
  ).toBeDisabled();
});

// --- Phase 3: mass-select / bulk-delete / clear-all ---

test('checking a row and clicking "Delete selected" POSTs bulk-delete with that id (after confirm)', async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
      "POST /api/emitters/bulk-delete": () => ({ deleted: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emitter emitter-1"));
  fireEvent.click(
    screen.getByRole("button", { name: /delete selected \(1\)/i }),
  );

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), "http://localhost").pathname ===
          "/api/emitters/bulk-delete" && init?.method === "POST",
    );
    expect(call).toBeDefined();
  });
  const [, init] = fetchMock.mock.calls.find(
    ([input]) =>
      new URL(String(input), "http://localhost").pathname ===
      "/api/emitters/bulk-delete",
  ) as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ ids: ["emitter-1"] });
});

test('"Clear All Emitters" POSTs clear after a confirm', async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
      "POST /api/emitters/clear": () => ({ deleted: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByRole("button", { name: /clear all emitters/i }));

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), "http://localhost").pathname ===
          "/api/emitters/clear" && init?.method === "POST",
    );
    expect(call).toBeDefined();
  });
});

test("declining the confirm dialog does not call bulk-delete", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(false);
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emitter emitter-1"));
  fireEvent.click(
    screen.getByRole("button", { name: /delete selected \(1\)/i }),
  );

  await waitFor(() => expect(window.confirm).toHaveBeenCalled());
  expect(
    fetchMock.mock.calls.find(([input]) =>
      String(input).includes("bulk-delete"),
    ),
  ).toBeUndefined();
});

test("clicking the row-level Delete button deletes just that emitter (after confirm)", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_UNASSIGNED], total: 1 }),
      "DELETE /api/emitters/emitter-1": () => ({}),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-1")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emitter-row-emitter-1");
  fireEvent.click(within(row).getByRole("button", { name: /^delete$/i }));

  await waitFor(() =>
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/emitters/emitter-1",
      expect.objectContaining({ method: "DELETE" }),
    ),
  );
});

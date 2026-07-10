import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, expect, test, vi } from "vitest";
import Emitters from "./Emitters";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { Emitter } from "../api/emitters";
import type { Entity } from "../api/entities";

afterEach(() => {
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>{children}</MemoryRouter>
    </QueryClientProvider>
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
 * a test doesn't care about them (the entities list, for the filter/associate
 * dropdowns), so each test only needs to override what it's actually
 * exercising. */
function baseRoutes(
  overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {},
) {
  return {
    "GET /api/entities": () => ({ items: [], total: 0 }),
    "GET /api/emitters/types": () => [],
    ...overrides,
  };
}

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
  emission_count: 0,
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
  emission_count: 0,
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
  emission_count: 42,
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
  emission_count: 7,
};

const ENTITY_1: Entity = {
  id: "entity-1",
  name: "Bob",
  notes: null,
  created_at: "2026-06-01T00:00:00Z",
};

/** The `wifi_access_point` emitter attribute catalog (Tasks 1-3 backend,
 * `GET /api/emitter-catalog/:emitter_type`) — used to exercise Task 4's
 * advanced attribute filter, scoped to the selected type. */
const WIFI_AP_ATTRIBUTE_CATALOG = [
  {
    key: "security",
    label: "Security",
    type: "text",
    ops: [{ code: "matches", label: "matches" }],
  },
];

const TYPE_OPTIONS = [
  { key: "wifi_client", label: "WiFi Client" },
  { key: "wifi_access_point", label: "WiFi Access Point" },
];

// --- Task 4: advanced attribute filter on the Emitters page ---

test("with a specific emitter_type selected, the attribute builder renders that type's fields", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_AP],
        total: 1,
      }),
      "GET /api/emitters/types": () => TYPE_OPTIONS,
      "GET /api/emitter-catalog/wifi_access_point": () =>
        WIFI_AP_ATTRIBUTE_CATALOG,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-4")).toBeInTheDocument(),
  );

  // Not shown for "All types" (the initial state).
  expect(screen.queryByTestId("condition-row-0")).not.toBeInTheDocument();

  fireEvent.change(screen.getByLabelText("Filter by type"), {
    target: { value: "wifi_access_point" },
  });

  const row = await screen.findByTestId("condition-row-0");
  expect(
    within(row).getByRole("option", { name: "Security" }),
  ).toBeInTheDocument();
});

test('with "All types" selected, the attribute builder is not rendered', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_AP], total: 1 }),
      "GET /api/emitters/types": () => TYPE_OPTIONS,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-4")).toBeInTheDocument(),
  );

  expect(screen.queryByTestId("condition-row-0")).not.toBeInTheDocument();
});

test("selecting a type then completing a field/op/value condition issues a listEmitters request carrying the cond param", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_AP], total: 1 }),
      "GET /api/emitters/types": () => TYPE_OPTIONS,
      "GET /api/emitter-catalog/wifi_access_point": () =>
        WIFI_AP_ATTRIBUTE_CATALOG,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-4")).toBeInTheDocument(),
  );

  fireEvent.change(screen.getByLabelText("Filter by type"), {
    target: { value: "wifi_access_point" },
  });

  const row = await screen.findByTestId("condition-row-0");
  const valueInput = row.querySelector(
    'input[id$="-value"]',
  ) as HTMLInputElement;
  fireEvent.change(valueInput, { target: { value: "WPA2" } });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.getAll("cond").includes("security:matches:\"WPA2\"")
      );
    });
    expect(call).toBeDefined();
  });
});

test("changing the type clears existing conditions", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_AP, EMITTER_CLIENT],
        total: 2,
      }),
      "GET /api/emitters/types": () => TYPE_OPTIONS,
      "GET /api/emitter-catalog/wifi_access_point": () =>
        WIFI_AP_ATTRIBUTE_CATALOG,
      "GET /api/emitter-catalog/wifi_client": () => [
        {
          key: "src_mac",
          label: "Source MAC",
          type: "mac",
          ops: [{ code: "eq", label: "is exactly" }],
        },
      ],
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emitter-row-emitter-4")).toBeInTheDocument(),
  );

  fireEvent.change(screen.getByLabelText("Filter by type"), {
    target: { value: "wifi_access_point" },
  });

  const row = await screen.findByTestId("condition-row-0");
  const valueInput = row.querySelector(
    'input[id$="-value"]',
  ) as HTMLInputElement;
  fireEvent.change(valueInput, { target: { value: "WPA2" } });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.getAll("cond").length > 0
      );
    });
    expect(call).toBeDefined();
  });

  // Switch types — the stale "security" condition must not survive onto the
  // new type's (unrelated) catalog, and no `cond` param should carry over.
  fireEvent.change(screen.getByLabelText("Filter by type"), {
    target: { value: "wifi_client" },
  });

  const newRow = await screen.findByTestId("condition-row-0");
  expect(
    within(newRow).getByRole("option", { name: "Source MAC" }),
  ).toBeInTheDocument();
  const newValueInput = newRow.querySelector(
    'input[id$="-value"]',
  ) as HTMLInputElement;
  expect(newValueInput.value).toBe("");

  await waitFor(() => {
    const calls = fetchMock.mock.calls.filter(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emitters" &&
        url.searchParams.get("emitter_type") === "wifi_client"
      );
    });
    expect(calls.length).toBeGreaterThan(0);
    for (const [input] of calls) {
      const url = new URL(String(input), "http://localhost");
      expect(url.searchParams.getAll("cond")).toEqual([]);
    }
  });
});

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

// --- Phase 3: name links to the dedicated detail page ---

test("emitter name links to its detail page", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  const link = await screen.findByRole("link", {
    name: "WiFi Client aa:bb:cc:dd:ee:ff",
  });
  expect(link).toHaveAttribute("href", "/emitters/emitter-3");
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

test('the Type filter offers "All types" plus the in-use types from the stable endpoint', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_CLIENT, EMITTER_AP],
        total: 2,
      }),
      "GET /api/emitters/types": () => [
        { key: "wifi_client", label: "WiFi Client" },
        { key: "wifi_access_point", label: "WiFi Access Point" },
      ],
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
  // Populated from `GET /api/emitters/types`, not derived from loaded rows.
  expect(
    within(typeSelect).getByRole("option", { name: "WiFi Client" }),
  ).toHaveValue("wifi_client");
  expect(
    within(typeSelect).getByRole("option", { name: "WiFi Access Point" }),
  ).toHaveValue("wifi_access_point");
});

test("shows the emission count column and sorts when a header is clicked", async () => {
  let lastEmittersUrl: URL | null = null;
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": (url) => {
        lastEmittersUrl = url;
        return { items: [EMITTER_CLIENT], total: 1 };
      },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });
  expect(await screen.findByText("42")).toBeInTheDocument();

  // Click the "Last Seen" header -> toggles to ascending (default is desc).
  await userEvent.click(screen.getByRole("button", { name: /Last Seen/ }));

  // The most recent listEmitters call carries sort=last_seen & dir=asc.
  await waitFor(() => {
    expect(lastEmittersUrl?.searchParams.get("sort")).toBe("last_seen");
    expect(lastEmittersUrl?.searchParams.get("dir")).toBe("asc");
  });
});

test("populates the Type filter from the in-use endpoint, stable across selection", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({ items: [EMITTER_CLIENT], total: 1 }),
      "GET /api/emitters/types": () => [
        { key: "wifi_client", label: "WiFi Client" },
        { key: "bluetooth_device", label: "Bluetooth Device" },
      ],
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emitters />, { wrapper });

  // Both options are present even though the only loaded row is wifi_client.
  expect(
    await screen.findByRole("option", { name: "Bluetooth Device" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("option", { name: "WiFi Client" }),
  ).toBeInTheDocument();
});

test("choosing a type in the Type filter refetches with the emitter_type param", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitters": () => ({
        items: [EMITTER_CLIENT, EMITTER_AP],
        total: 2,
      }),
      "GET /api/emitters/types": () => [
        { key: "wifi_client", label: "WiFi Client" },
        { key: "wifi_access_point", label: "WiFi Access Point" },
      ],
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

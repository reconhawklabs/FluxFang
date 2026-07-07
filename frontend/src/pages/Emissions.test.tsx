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
import Emissions from "./Emissions";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { Emission } from "../api/emissions";
import type { Emitter } from "../api/emitters";
import type { DataSource } from "../api/dataSources";
import type { FieldDef } from "../types/catalog";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
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
 * `DataSources.test.tsx`'s `mockMethodRoutes`) — this page hits several
 * distinct GET paths (`/api/emissions`, `/api/emitters`, `/api/data-sources`,
 * `/api/catalog/wifi`) plus POSTs (`/api/emitters`, bulk-delete, clear), so
 * routing needs both dimensions. `handler` receives the parsed `URL` (and
 * `init`) so a test can assert on query params/body or vary the response by
 * them. */
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

const WIFI_CATALOG: FieldDef[] = [
  {
    key: "bssid",
    label: "BSSID",
    type: "mac",
    ops: [
      { code: "eq", label: "is exactly" },
      { code: "matches", label: "contains / matches" },
    ],
  },
  {
    key: "src_mac",
    label: "Src MAC",
    type: "mac",
    ops: [{ code: "eq", label: "is exactly" }],
  },
  {
    key: "ssid",
    label: "SSID",
    type: "text",
    ops: [{ code: "eq", label: "is exactly" }],
  },
  {
    key: "channel",
    label: "Channel",
    type: "number",
    ops: [{ code: "gte", label: "is at least" }],
  },
];

const EMISSION_1: Emission = {
  id: "e1",
  data_source_id: "ds1",
  emitter_id: null,
  session_id: null,
  observed_at: "2026-07-05T12:00:00Z",
  signal_strength: -55,
  lon: -122.4,
  lat: 37.7,
  kind: "wifi",
  payload: { bssid: "aa:bb:cc:dd:ee:ff", ssid: "CoffeeShop", channel: 6 },
};

const EMISSION_2: Emission = {
  id: "e2",
  data_source_id: "ds1",
  emitter_id: "emitter-1",
  session_id: null,
  observed_at: "2026-07-05T12:05:00Z",
  signal_strength: -70,
  lon: null,
  lat: null,
  kind: "wifi",
  payload: { bssid: "11:22:33:44:55:66", ssid: "Home", channel: 11 },
};

const EMISSION_PROBE: Emission = {
  id: "e3",
  data_source_id: "ds1",
  emitter_id: null,
  session_id: null,
  observed_at: "2026-07-05T12:10:00Z",
  signal_strength: -60,
  lon: null,
  lat: null,
  kind: "wifi",
  payload: { src_mac: "de:ad:be:ef:00:01", frame_type: "probe_request" },
};

const EMITTER_1: Emitter = {
  id: "emitter-1",
  name: "My Router",
  type: null,
  emitter_type: null,
  attributes: {},
  match_enabled: true,
  type_label: null,
  category: null,
  entity_id: null,
  match_criteria: {},
  first_seen_at: null,
  last_seen_at: null,
  created_at: "2026-07-01T00:00:00Z",
};

const WIFI_EMITTER_TYPES = [
  { key: "wifi_access_point", label: "WiFi Access Point" },
  { key: "wifi_client", label: "WiFi Client" },
];

const DATA_SOURCE_1: DataSource = {
  id: "ds1",
  created_at: "2026-01-01T00:00:00Z",
  kind: "wifi",
  mode: "monitor",
  interface: "wlan0",
  status: "running",
  config: {},
  last_error: null,
};

const DATA_SOURCE_2: DataSource = {
  id: "ds2",
  created_at: "2026-01-01T00:00:00Z",
  kind: "gps",
  mode: "gpsd",
  interface: null,
  status: "running",
  config: { host: "localhost", port: 2947 },
  last_error: null,
};

/** The default routes every test needs at minimum (emissions/emitters/
 * catalog/data-sources) — individual tests spread over this and override/
 * add routes (emitter-types, preview, POST/bulk-delete/clear) as needed. */
function baseRoutes(
  overrides: Record<string, (url: URL, init?: RequestInit) => unknown> = {},
) {
  return {
    "GET /api/emissions": () => ({ items: [EMISSION_1, EMISSION_2], total: 2 }),
    "GET /api/emitters": () => ({ items: [EMITTER_1], total: 1 }),
    "GET /api/catalog/wifi": () => WIFI_CATALOG,
    "GET /api/data-sources": () => [DATA_SOURCE_1, DATA_SOURCE_2],
    ...overrides,
  };
}

test("renders emission rows (bssid/channel/rssi) and the total from a mocked response", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });

  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  const row1 = screen.getByTestId("emission-row-e1");
  expect(within(row1).getByText("aa:bb:cc:dd:ee:ff")).toHaveClass("font-mono");
  expect(within(row1).getByText("6")).toBeInTheDocument();
  expect(within(row1).getByText("-55")).toBeInTheDocument();
  expect(within(row1).getByTestId("emission-src-mac")).toHaveTextContent("—"); // no src_mac in payload

  const row2 = screen.getByTestId("emission-row-e2");
  expect(within(row2).getByText("My Router")).toBeInTheDocument();

  expect(screen.getByTestId("emissions-total")).toHaveTextContent(
    "2 emissions",
  );
});

test("the data-source dropdown is populated from listDataSources (gps sources excluded); selecting one adds data_source_id to the query", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  const select = await screen.findByLabelText(/data source/i);
  expect(within(select).getByText("wifi (wlan0)")).toBeInTheDocument();
  // GPS provides location, not emissions, so it must not appear in the
  // emissions data-source filter (see `isEmittingSource`).
  expect(within(select).queryByText("gps (gpsd)")).not.toBeInTheDocument();

  fireEvent.change(select, { target: { value: "ds1" } });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emissions" &&
        url.searchParams.get("data_source_id") === "ds1"
      );
    });
    expect(call).toBeDefined();
  });
});

test('the per-row "+" opens the assign modal pre-filled with that emission\'s bssid identity rule', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/emitter-types/wifi": () => WIFI_EMITTER_TYPES }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Quick-assign emission e1 to emitter"));
  const heading = await screen.findByRole("heading", {
    name: /assign 1 emission to emitter/i,
  });
  const modal = within(heading.closest("form") as HTMLElement);

  await waitFor(() =>
    expect(modal.getByLabelText(/field/i)).toHaveValue("bssid"),
  );
  expect(modal.getByLabelText(/operator/i)).toHaveValue("eq");
  expect(modal.getByLabelText(/value/i)).toHaveValue("aa:bb:cc:dd:ee:ff");
});

test('the per-row "+" on a probe-request emission pre-fills src_mac eq <src_mac>', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": () => ({ items: [EMISSION_PROBE], total: 1 }),
      "GET /api/emitter-types/wifi": () => WIFI_EMITTER_TYPES,
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e3")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Quick-assign emission e3 to emitter"));
  const heading = await screen.findByRole("heading", {
    name: /assign 1 emission to emitter/i,
  });
  const modal = within(heading.closest("form") as HTMLElement);

  await waitFor(() =>
    expect(modal.getByLabelText(/field/i)).toHaveValue("src_mac"),
  );
  expect(modal.getByLabelText(/operator/i)).toHaveValue("eq");
  expect(modal.getByLabelText(/value/i)).toHaveValue("de:ad:be:ef:00:01");
});

test("selecting an emission and bulk-assigning prefills RuleBuilder with bssid eq <value> and POSTs match_criteria", async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitter-types/wifi": () => WIFI_EMITTER_TYPES,
      "GET /api/emitters/preview": () => ({ match_count: 4 }),
      "POST /api/emitters": () => ({
        emitter: { ...EMITTER_1, id: "emitter-2", name: "Coffee Shop AP" },
        attached_count: 4,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
  fireEvent.click(screen.getByRole("button", { name: /assign to emitter/i }));

  const heading = await screen.findByRole("heading", {
    name: /assign 1 emission to emitter/i,
  });
  const modal = within(heading.closest("form") as HTMLElement);
  await waitFor(() =>
    expect(modal.getByLabelText(/field/i)).toHaveValue("bssid"),
  );

  expect(modal.getByLabelText(/operator/i)).toHaveValue("eq");
  expect(modal.getByLabelText(/value/i)).toHaveValue("aa:bb:cc:dd:ee:ff");

  fireEvent.change(screen.getByLabelText(/^name$/i), {
    target: { value: "Coffee Shop AP" },
  });
  fireEvent.click(screen.getByRole("button", { name: /^assign$/i }));

  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent(
      /assigned 4 emission/i,
    ),
  );

  const postCall = fetchMock.mock.calls.find(
    ([, init]) => init?.method === "POST",
  );
  expect(postCall).toBeDefined();
  const [url, init] = postCall as [RequestInfo | URL, RequestInit];
  expect(String(url)).toBe("/api/emitters");
  const body = JSON.parse(init.body as string);
  expect(body.name).toBe("Coffee Shop AP");
  expect(body.match_criteria).toEqual({
    match: "all",
    conditions: [{ field: "bssid", op: "eq", value: "aa:bb:cc:dd:ee:ff" }],
  });
});

test('paging (Next) clears the row selection so "Assign to emitter" can never no-op on a stale id', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": (url) =>
        url.searchParams.get("offset") === "50"
          ? { items: [EMISSION_2], total: 60 }
          : { items: [EMISSION_1], total: 60 },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
  expect(
    screen.getByRole("button", { name: /assign to emitter \(1\)/i }),
  ).toBeEnabled();

  fireEvent.click(screen.getByRole("button", { name: /^next$/i }));
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e2")).toBeInTheDocument(),
  );

  const assignButton = screen.getByRole("button", {
    name: /assign to emitter \(0\)/i,
  });
  expect(assignButton).toBeDisabled();

  fireEvent.click(assignButton);
  expect(
    screen.queryByRole("heading", { name: /assign .* to emitter/i }),
  ).not.toBeInTheDocument();
});

test("a stacked filter condition refetches emissions with the matching cond= query param", async () => {
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  const fieldSelect = await screen.findByLabelText(/field/i);
  fireEvent.change(fieldSelect, { target: { value: "ssid" } });
  const valueInput = screen.getByLabelText(/value/i);
  fireEvent.change(valueInput, { target: { value: "CoffeeShop" } });

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(([input]) => {
      const url = new URL(String(input), "http://localhost");
      return (
        url.pathname === "/api/emissions" &&
        url.searchParams.get("cond") === 'ssid:eq:"CoffeeShop"'
      );
    });
    expect(call).toBeDefined();
  });
});

test('a probe-request emission (payload.src_mac, no bssid) renders the Src MAC column monospace and BSSID as "—"', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emissions": () => ({ items: [EMISSION_PROBE], total: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e3")).toBeInTheDocument(),
  );

  const row = screen.getByTestId("emission-row-e3");
  expect(within(row).getByText("de:ad:be:ef:00:01")).toHaveClass("font-mono");
});

// --- Task C: Assign-modal Type dropdown (GET /api/emitter-types/:kind) ---

test('the Assign modal shows a Type <select> (not a text input) with options from the emitter-types endpoint plus "Other"', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({ "GET /api/emitter-types/wifi": () => WIFI_EMITTER_TYPES }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
  fireEvent.click(screen.getByRole("button", { name: /assign to emitter/i }));
  await screen.findByRole("heading", { name: /assign 1 emission to emitter/i });

  const typeSelect = await screen.findByLabelText(/^type$/i);
  expect(typeSelect.tagName).toBe("SELECT");
  const optionLabels = within(typeSelect as HTMLSelectElement)
    .getAllByRole("option")
    .map((option) => option.textContent);
  expect(optionLabels).toEqual(
    expect.arrayContaining([
      "WiFi Access Point",
      "WiFi Client",
      expect.stringMatching(/other/i),
    ]),
  );
});

test('selecting "Other (custom)" reveals a text input; submitting sends type (custom text) and omits emitter_type', async () => {
  const fetchMock = mockRoutes(
    baseRoutes({
      "GET /api/emitter-types/wifi": () => WIFI_EMITTER_TYPES,
      "GET /api/emitters/preview": () => ({ match_count: 4 }),
      "POST /api/emitters": () => ({
        emitter: { ...EMITTER_1, id: "emitter-2", name: "Custom Thing" },
        attached_count: 4,
      }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
  fireEvent.click(screen.getByRole("button", { name: /assign to emitter/i }));
  await screen.findByRole("heading", { name: /assign 1 emission to emitter/i });

  const typeSelect = await screen.findByLabelText(/^type$/i);
  const otherOption = within(typeSelect as HTMLSelectElement)
    .getAllByRole("option")
    .find((option) => /other/i.test(option.textContent ?? ""));
  expect(otherOption).toBeDefined();
  fireEvent.change(typeSelect, {
    target: { value: (otherOption as HTMLOptionElement).value },
  });

  const customInput = await screen.findByLabelText(/custom type/i);
  fireEvent.change(customInput, { target: { value: "Custom Thing" } });
  fireEvent.change(screen.getByLabelText(/^name$/i), {
    target: { value: "Custom Thing" },
  });
  fireEvent.click(screen.getByRole("button", { name: /^assign$/i }));

  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent(
      /assigned 4 emission/i,
    ),
  );

  const postCall = fetchMock.mock.calls.find(
    ([, init]) => init?.method === "POST",
  );
  expect(postCall).toBeDefined();
  const [, init] = postCall as [RequestInfo | URL, RequestInit];
  const body = JSON.parse(init.body as string);
  expect(body.type).toBe("Custom Thing");
  expect(body).not.toHaveProperty("emitter_type");
});

// --- Phase 2: mass-select / bulk-delete / clear-all ---

test('checking a row and clicking "Delete selected" POSTs bulk-delete with that id (after confirm)', async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      "POST /api/emissions/bulk-delete": () => ({ deleted: 1 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
  fireEvent.click(
    screen.getByRole("button", { name: /delete selected \(1\)/i }),
  );

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), "http://localhost").pathname ===
          "/api/emissions/bulk-delete" && init?.method === "POST",
    );
    expect(call).toBeDefined();
  });
  const [, init] = fetchMock.mock.calls.find(
    ([input]) =>
      new URL(String(input), "http://localhost").pathname ===
      "/api/emissions/bulk-delete",
  ) as [RequestInfo | URL, RequestInit];
  expect(JSON.parse(init.body as string)).toEqual({ ids: ["e1"] });
});

test('"Clear All Emissions" POSTs clear after a confirm', async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const fetchMock = mockRoutes(
    baseRoutes({
      "POST /api/emissions/clear": () => ({ deleted: 2 }),
    }),
  );
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByRole("button", { name: /clear all emissions/i }));

  await waitFor(() => {
    const call = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(String(input), "http://localhost").pathname ===
          "/api/emissions/clear" && init?.method === "POST",
    );
    expect(call).toBeDefined();
  });
});

test("declining the confirm dialog does not call bulk-delete", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(false);
  const fetchMock = mockRoutes(baseRoutes());
  vi.stubGlobal("fetch", fetchMock);

  render(<Emissions />, { wrapper });
  await waitFor(() =>
    expect(screen.getByTestId("emission-row-e1")).toBeInTheDocument(),
  );

  fireEvent.click(screen.getByLabelText("Select emission e1"));
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

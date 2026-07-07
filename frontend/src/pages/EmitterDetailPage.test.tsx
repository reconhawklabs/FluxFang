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

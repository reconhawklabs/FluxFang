import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, expect, test, vi } from "vitest";
import EntityDetailPage from "./EntityDetailPage";
import { jsonResponse } from "../test-utils/fetchMocks";
import type { EntityDetail } from "../api/entities";

// EmissionsHeatmap inits a real MapLibre map for non-empty points — mock it,
// same convention as EmitterDetailPage.test.tsx.
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

const ENTITY_DETAIL: EntityDetail = {
  id: "entity-1",
  name: "Bob's Phone",
  notes: null,
  created_at: "2026-07-06T00:00:00Z",
  last_seen: "2026-07-06T01:00:00Z",
  recent_detections: [
    {
      emitter_id: null,
      lon: -98.5,
      lat: 39.5,
      observed_at: "2026-07-06T01:00:00Z",
    },
  ],
  emitters: [
    {
      id: "em1",
      name: "Phone Wi-Fi",
      type: "wifi_client",
      type_label: "wifi_client",
      emitter_type: "wifi_client",
      entity_id: "entity-1",
      match_enabled: true,
      match_criteria: { match: "all", conditions: [] },
      attributes: {},
      first_seen_at: "2026-07-06T00:00:00Z",
      last_seen_at: "2026-07-06T01:00:00Z",
    } as unknown as EntityDetail["emitters"][number],
  ],
};

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
      <MemoryRouter initialEntries={["/entities/entity-1"]}>
        <Routes>
          <Route path="/entities/:id" element={children} />
          <Route path="/entities" element={<div>Entities list</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<EntityDetailPage />, { wrapper: Wrapper });
}

test("renders the entity name, last-seen and associated emitters", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({
      "GET /api/entities/entity-1": () => ENTITY_DETAIL,
      "GET /api/alert-rules": () => [],
    }),
  );
  renderPage();
  expect(
    await screen.findByRole("heading", { name: /bob's phone/i }),
  ).toBeInTheDocument();
  expect(screen.getByText(/phone wi-fi/i)).toBeInTheDocument();
});

test("shows not-found on a 404", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const raw = typeof input === "string" ? input : input.toString();
      const url = new URL(raw, "http://localhost");
      if (url.pathname === "/api/entities/entity-1") {
        return Promise.resolve(jsonResponse({ message: "not found" }, 404));
      }
      if (url.pathname === "/api/alert-rules") {
        return Promise.resolve(jsonResponse([]));
      }
      return Promise.reject(new Error(`no route for ${url.pathname}`));
    }),
  );
  renderPage();
  expect(await screen.findByText(/entity not found/i)).toBeInTheDocument();
});

import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, expect, test, vi } from "vitest";
import ZoneDetailPage from "./ZoneDetailPage";
import { jsonResponse } from "../test-utils/fetchMocks";

afterEach(() => vi.unstubAllGlobals());

const ZONE_DETAIL = {
  id: "zone-1",
  name: "HQ",
  lat: 39.5,
  lon: -98.5,
  radius_m: 200,
  notes: "Main office",
  created_at: "2026-01-01T00:00:00Z",
  emitters: [{ id: "em1", name: "Lobby AP", type: "wifi_access_point" }],
  entities: [{ id: "en1", name: "Bob's Phone" }],
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
      <MemoryRouter initialEntries={["/zones/zone-1"]}>
        <Routes>
          <Route path="/zones/:id" element={children} />
          <Route path="/zones" element={<div>Zones list</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<ZoneDetailPage />, { wrapper: Wrapper });
}

test("renders the zone name, center and subjects", async () => {
  vi.stubGlobal(
    "fetch",
    mockRoutes({ "GET /api/zones/zone-1": () => ZONE_DETAIL }),
  );
  renderPage();
  expect(
    await screen.findByRole("heading", { name: /hq/i }),
  ).toBeInTheDocument();
  expect(screen.getByText(/lobby ap/i)).toBeInTheDocument();
  expect(screen.getByText(/bob's phone/i)).toBeInTheDocument();
});

test("shows not-found on a 404", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(jsonResponse({ message: "not found" }, 404))),
  );
  renderPage();
  expect(await screen.findByText(/zone not found/i)).toBeInTheDocument();
});

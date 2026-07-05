// Shared `fetch` mock helpers for component tests (`RuleBuilder`,
// `FilterBar`, `useCatalog`, Task 9.2) — factors out the boilerplate
// `Response`-shaped object every test in this repo builds by hand (see
// `pages/Login.test.tsx`) plus URL-routing across more than one endpoint
// (catalog + preview), which none of the existing single-endpoint tests
// needed.
import { vi } from 'vitest';

/** A minimal object shaped like the `fetch` `Response` methods
 * `src/api/client.ts`'s `request()` actually calls (`ok`, `status`,
 * `statusText`, `text()`, `clone()`, `json()`). */
export function jsonResponse(body: unknown, status = 200): Response {
  const text = JSON.stringify(body);
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    text: async () => text,
    clone() {
      return this;
    },
    json: async () => JSON.parse(text),
  } as unknown as Response;
}

/** Builds a `vi.fn` standing in for global `fetch`, resolving by the
 * request URL's pathname (query string ignored for routing, but available
 * to a handler function for e.g. asserting on `?rule=`). `routes` maps
 * pathname -> either a fixed JSON body or a `(url) => body` function for
 * routes whose response depends on query params. Throws (rather than
 * hanging) on any pathname not registered, so an unexpected request fails
 * the test loudly instead of silently resolving `undefined`. */
export function mockFetchRoutes(routes: Record<string, unknown | ((url: URL) => unknown)>) {
  return vi.fn((input: RequestInfo | URL) => {
    const raw = typeof input === 'string' ? input : input.toString();
    const url = new URL(raw, 'http://localhost');
    const handler = routes[url.pathname];
    if (handler === undefined) {
      return Promise.reject(new Error(`mockFetchRoutes: no route registered for ${url.pathname}`));
    }
    const body = typeof handler === 'function' ? (handler as (url: URL) => unknown)(url) : handler;
    return Promise.resolve(jsonResponse(body));
  });
}

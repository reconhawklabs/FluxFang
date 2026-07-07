import { expect, test } from "vitest";
import { isEmittingSource } from "./dataSources";
import type { DataSource } from "./dataSources";

function source(kind: DataSource["kind"]): DataSource {
  return {
    id: "s1",
    created_at: "2026-07-06T00:00:00Z",
    kind,
    mode: kind === "gps" ? "gpsd" : "monitor",
    interface: kind === "gps" ? null : "wlan0",
    status: "running",
    config: {},
    last_error: null,
  };
}

test("isEmittingSource is false for gps, true for wifi", () => {
  expect(isEmittingSource(source("gps"))).toBe(false);
  expect(isEmittingSource(source("wifi"))).toBe(true);
});

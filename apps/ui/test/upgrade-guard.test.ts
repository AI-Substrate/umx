import { describe, expect, it } from "vitest";
import { upgradeAllowed } from "../bin/upgrade-guard.mjs";

const port = 4321;
const ok = { host: "127.0.0.1:4321", origin: "http://127.0.0.1:4321" };

describe("dev WebSocket upgrades (security F1)", () => {
  it("passes this run's own page", () => {
    expect(upgradeAllowed(ok, port)).toBe(true);
    expect(upgradeAllowed({ host: "localhost:4321", origin: "http://localhost:4321" }, port)).toBe(true);
  });

  it("refuses another site, a null or missing Origin, and a wrong Host", () => {
    for (const origin of ["http://evil.example", "null", "http://127.0.0.1:9999", "https://127.0.0.1:4321", undefined]) {
      expect(upgradeAllowed({ ...ok, origin }, port)).toBe(false);
    }
    for (const host of ["evil.example:4321", "localhost:4321", "127.0.0.1:9999", "[::1]:4321", undefined]) {
      expect(upgradeAllowed({ ...ok, host }, port)).toBe(false);
    }
    // Each loopback name only with its own Origin.
    expect(upgradeAllowed({ host: "localhost:4321", origin: "http://127.0.0.1:4321" }, port)).toBe(false);
  });
});

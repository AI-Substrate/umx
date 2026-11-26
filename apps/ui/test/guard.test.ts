import { describe, expect, it } from "vitest";
import { guard, readGuardConfig, type GuardConfig, type GuardInput } from "@/lib/guard";

const cfg: GuardConfig = { port: 4321, dev: false };
/** The view's own API call: same origin, with the view header. No token, no cookie. */
const api = (over: Partial<GuardInput> = {}): GuardInput => ({
  method: "GET",
  pathname: "/api/graphs",
  host: "127.0.0.1:4321",
  origin: null,
  secFetchSite: "same-origin",
  view: "1",
  ...over,
});
const refused = { ok: false, status: 403 };

describe("guard", () => {
  it("accepts only configured LAN addresses on the public port and preserves every API origin check", () => {
    const lan = { ...cfg, lanHosts: ["192.168.1.10:4321"] };
    const request = api({ host: "192.168.1.10:4321", origin: "http://192.168.1.10:4321" });
    expect(guard(request, cfg)).toMatchObject(refused);
    expect(guard(request, lan)).toEqual({ ok: true });
    expect(guard({ ...request, origin: "http://evil.example" }, lan)).toMatchObject(refused);
    expect(guard({ ...request, secFetchSite: "cross-site" }, lan)).toMatchObject(refused);
    expect(guard({ ...request, view: null }, lan)).toMatchObject(refused);
    expect(guard({ ...request, host: "192.168.1.11:4321" }, lan)).toMatchObject(refused);
  });

  it("serves a plain same-origin request with no token or cookie, on 127.0.0.1 and localhost", () => {
    expect(guard(api(), cfg)).toEqual({ ok: true });
    expect(guard(api({ pathname: "/", secFetchSite: "none", view: null }), cfg)).toEqual({ ok: true });
    expect(guard(api({ host: "localhost:4321" }), cfg)).toEqual({ ok: true });
    const post = api({ method: "POST", pathname: "/api/graphs/demo/selection", origin: "http://127.0.0.1:4321" });
    expect(guard(post, cfg)).toEqual({ ok: true });
    expect(guard({ ...post, host: "localhost:4321", origin: "http://localhost:4321" }, cfg)).toEqual({ ok: true });
  });

  it("refuses a Host that is not this port's loopback name with 403 and no detail, static assets included (DNS rebinding)", () => {
    for (const host of ["evil.example:4321", "127.0.0.1:9999", "localhost:9999", "0.0.0.0:4321", "[::1]:4321", "127.0.0.1", "localhost", null]) {
      for (const pathname of ["/", "/_next/static/chunks/app.js", "/api/graphs", "/api/launch"]) {
        expect(guard(api({ host, pathname }), cfg)).toEqual({ ...refused, reason: "Refused." });
      }
    }
  });

  it("refuses a foreign or null Origin on every path and method", () => {
    for (const origin of ["http://evil.example", "http://127.0.0.1:9999", "https://127.0.0.1:4321", "null", "http://localhost:4321"]) {
      expect(guard(api({ origin }), cfg)).toMatchObject(refused);
      expect(guard(api({ method: "POST", pathname: "/api/launch", origin }), cfg)).toMatchObject(refused);
      expect(guard(api({ pathname: "/", origin }), cfg)).toMatchObject(refused);
    }
  });

  it("refuses API requests that say they are cross-site, the event stream included", () => {
    for (const site of ["cross-site", "same-site", "none"]) {
      expect(guard(api({ secFetchSite: site }), cfg)).toMatchObject(refused);
      expect(guard(api({ pathname: "/api/events", secFetchSite: site }), cfg)).toMatchObject(refused);
      const post = api({ method: "POST", pathname: "/api/graphs/demo/selection", origin: "http://127.0.0.1:4321", secFetchSite: site });
      expect(guard(post, cfg)).toMatchObject(refused);
    }
    // An older browser sends no Sec-Fetch-Site: the view header still says the call is the view's own.
    expect(guard(api({ secFetchSite: null }), cfg)).toEqual({ ok: true });
  });

  it("refuses API requests without the view header, which no cross-site page can send without a preflight", () => {
    for (const view of [null, "", "0", "true"]) {
      expect(guard(api({ view, secFetchSite: null }), cfg)).toMatchObject(refused);
      expect(guard(api({ view, pathname: "/api/events", secFetchSite: null }), cfg)).toMatchObject(refused);
    }
  });

  it("refuses an API write with no Origin", () => {
    for (const method of ["POST", "PUT", "DELETE", "PATCH"]) {
      expect(guard(api({ method, pathname: "/api/launch" }), cfg)).toMatchObject(refused);
    }
  });

  it("refuses everything when not started by bin/serve.mjs", () => {
    expect(readGuardConfig({})).toBeNull();
    expect(readGuardConfig({ UMX_UI_PORT: "0" })).toBeNull();
    expect(readGuardConfig({ UMX_UI_PORT: "4321" })).toEqual({ port: 4321, dev: false });
    expect(guard(api(), null)).toMatchObject({ ok: false, status: 503 });
  });
});

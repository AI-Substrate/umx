import { describe, expect, it } from "vitest";
import { guard, readGuardConfig, safeEqual, type GuardConfig, type GuardInput } from "@/lib/guard";

const cfg: GuardConfig = { port: 4321, token: "t".repeat(43), session: "s".repeat(43), cookieName: "umx_ui_ab12", dev: false };
const api = (over: Partial<GuardInput> = {}): GuardInput => ({
  method: "GET",
  pathname: "/api/graphs",
  host: "127.0.0.1:4321",
  origin: null,
  secFetchSite: "same-origin",
  cookie: cfg.session,
  ...over,
});

describe("guard", () => {
  it("lets this run's browser read the API", () => {
    expect(guard(api(), cfg)).toEqual({ ok: true });
  });

  it("refuses a wrong Host with 403 on every path, static assets included", () => {
    for (const host of ["evil.example:4321", "localhost:4321", "127.0.0.1:9999", "0.0.0.0:4321", "[::1]:4321", null]) {
      for (const pathname of ["/", "/_next/static/chunks/app.js", "/api/graphs", "/api/session"]) {
        expect(guard(api({ host, pathname }), cfg)).toMatchObject({ ok: false, status: 403 });
      }
    }
  });

  it("refuses an API request without this run's cookie", () => {
    expect(guard(api({ cookie: undefined }), cfg)).toMatchObject({ ok: false, status: 401 });
    expect(guard(api({ cookie: "s".repeat(42) + "x" }), cfg)).toMatchObject({ ok: false, status: 401 });
    expect(guard(api({ cookie: cfg.token }), cfg)).toMatchObject({ ok: false, status: 401 });
  });

  it("refuses a foreign or null Origin, even with the cookie", () => {
    for (const origin of ["http://evil.example", "http://127.0.0.1:9999", "https://127.0.0.1:4321", "null"]) {
      expect(guard(api({ origin }), cfg)).toMatchObject({ ok: false, status: 403 });
      expect(guard(api({ method: "POST", pathname: "/api/session", origin }), cfg)).toMatchObject({ ok: false, status: 403 });
      expect(guard(api({ pathname: "/", origin }), cfg)).toMatchObject({ ok: false, status: 403 });
    }
  });

  it("refuses API GETs that say they are cross-site", () => {
    for (const site of ["cross-site", "same-site", "none"]) {
      expect(guard(api({ secFetchSite: site }), cfg)).toMatchObject({ ok: false, status: 403 });
    }
    expect(guard(api({ secFetchSite: null }), cfg)).toEqual({ ok: true });
  });

  it("refuses API POSTs that say they are cross-site, even from the right Origin with the cookie", () => {
    const post = api({ method: "POST", pathname: "/api/graphs/demo/selection", origin: "http://127.0.0.1:4321" });
    expect(guard(post, cfg)).toEqual({ ok: true });
    for (const site of ["cross-site", "same-site", "none"]) {
      expect(guard({ ...post, secFetchSite: site }, cfg)).toMatchObject({ ok: false, status: 403 });
    }
    expect(guard({ ...post, origin: null }, cfg)).toMatchObject({ ok: false, status: 403 });
    expect(guard({ ...post, cookie: undefined }, cfg)).toMatchObject({ ok: false, status: 401 });
  });

  it("takes the token exchange only as a same-origin POST", () => {
    const post = api({ method: "POST", pathname: "/api/session", cookie: undefined, origin: "http://127.0.0.1:4321" });
    expect(guard(post, cfg)).toEqual({ ok: true });
    expect(guard({ ...post, origin: null }, cfg)).toMatchObject({ ok: false, status: 403 });
    expect(guard({ ...post, method: "GET", origin: null }, cfg)).toMatchObject({ ok: false, status: 401 });
  });

  it("serves the page shell without the cookie, so the token can be exchanged", () => {
    expect(guard(api({ pathname: "/", cookie: undefined, secFetchSite: "none" }), cfg)).toEqual({ ok: true });
  });

  it("refuses everything when not started by bin/serve.mjs", () => {
    expect(readGuardConfig({})).toBeNull();
    expect(readGuardConfig({ UMX_UI_PORT: "4321", UMX_UI_TOKEN: "short", UMX_UI_SESSION: "s".repeat(43), UMX_UI_COOKIE: "c" })).toBeNull();
    expect(guard(api(), null)).toMatchObject({ ok: false, status: 503 });
  });

  it("compares secrets fully", () => {
    expect(safeEqual("abc", "abc")).toBe(true);
    expect(safeEqual("abd", "abc")).toBe(false);
    expect(safeEqual("ab", "abc")).toBe(false);
    expect(safeEqual("abcd", "abc")).toBe(false);
    expect(safeEqual("", "abc")).toBe(false);
  });
});

import { chmodSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable } from "node:stream";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "@/lib/api";
import { lanGate as gateAt, LIFETIME, LOGIN, SIGNED_OUT } from "../bin/lan.mjs";

const password = "6A3J-DJ8A-YCK3";
const host = "192.168.1.10:4321";
const homes: string[] = [];

/** A private UMX_HOME holding `value` as its owner-only password file. */
function home(value = password) {
  const dir = mkdtempSync(join(tmpdir(), "umx-lan-"));
  homes.push(dir);
  setPassword(dir, value);
  return dir;
}

/** Replaces the password file the way `umx ui lan-password --rotate` does: a new file renamed over the old. */
function setPassword(dir: string, value: string) {
  const temporary = join(dir, ".ui-lan-password-test");
  writeFileSync(temporary, `${value}\n`, { mode: 0o600 });
  renameSync(temporary, join(dir, "ui-lan-password"));
}

let shared = "";
function lanGate(config: { hosts: string[]; now?: () => number; home?: string }) {
  shared ||= home();
  return gateAt({ home: config.home ?? shared, hosts: config.hosts, now: config.now });
}

afterEach(() => {
  for (const dir of homes.splice(0)) rmSync(dir, { recursive: true, force: true });
  shared = "";
  vi.unstubAllGlobals();
});

async function visit(gate: ReturnType<typeof lanGate>, {
  ip = "192.168.1.20", path = "/", method = "GET", headers = {}, body = "",
}: { ip?: string; path?: string; method?: string; headers?: Record<string, string>; body?: string } = {}) {
  const req = Object.assign(Readable.from([Buffer.from(body)]), {
    method, url: path, headers: { host, ...headers }, socket: { remoteAddress: ip },
  });
  let status = 200;
  let response = "";
  let responseHeaders: Record<string, string> = {};
  const res = {
    writeHead(code: number, values: Record<string, string>) { status = code; responseHeaders = values; },
    end(text: string) { response = text; },
  };
  const admitted = await gate(req as IncomingMessage, res as ServerResponse);
  return { admitted, status, response, headers: responseHeaders };
}

const login = (value: string) => ({
  path: LOGIN, method: "POST", body: new URLSearchParams({ password: value }).toString(),
  headers: { origin: `http://${host}`, "sec-fetch-site": "same-origin" },
});

describe("LAN admission", () => {
  it("requires a password for every remote path and admits a random, host-bound session", async () => {
    const gate = lanGate({ hosts: [host, "192.168.1.11:4321"] });
    for (const path of ["/", "/api/graphs", "/api/events", "/_next/static/app.js"]) {
      const res = await visit(gate, { path });
      expect(res.status).toBe(401);
      expect(res.response).toContain('type="password"');
      expect(res.headers["cache-control"]).toBe("no-store");
      expect(res.headers["referrer-policy"]).toBe("same-origin");
    }
    const signed = await visit(gate, login(password));
    expect(signed.status).toBe(303);
    expect(signed.headers["set-cookie"]).toMatch(/^umx-lan=[a-f0-9]{64}; Max-Age=2592000; HttpOnly; SameSite=Lax; Path=\/$/);
    const cookie = signed.headers["set-cookie"].split(";")[0];
    expect((await visit(gate, { path: "/api/events", headers: { cookie } })).admitted).toBe(true);
    expect((await visit(gate, { headers: { cookie, host: "192.168.1.11:4321" } })).status).toBe(401);
    expect((await visit(lanGate({ hosts: [host], home: home() }), { headers: { cookie } })).status).toBe(401);
    expect((await visit(gate, { headers: { cookie: "umx-lan=" + "a".repeat(64) } })).status).toBe(401);
  });

  it("refuses wrong passwords and caps concurrent attempts per real IP, then recovers after a minute", async () => {
    let time = 0;
    const gate = lanGate({ hosts: [host], now: () => time });
    const results = await Promise.all(Array.from({ length: 6 }, () => visit(gate, login("wrong"))));
    expect(results.map((r) => r.status).sort()).toEqual([403, 403, 403, 403, 403, 429]);
    expect((await visit(gate, login(password))).status).toBe(429);
    expect((await visit(gate, { ...login(password), ip: "192.168.1.21" })).status).toBe(303);
    time = 60_000;
    expect((await visit(gate, login(password))).status).toBe(303);
  });

  it("refuses mismatched password bytes without throwing and accepts only the exact password", async () => {
    const gate = lanGate({ hosts: [host] });
    for (const value of ["", `${password}A`, `\u00e9${password.slice(1)}`, `${password.slice(0, -1)}2`]) {
      const rejected = await visit(gate, login(value));
      expect(rejected.status).toBe(403);
      expect(rejected.headers["set-cookie"]).toBeUndefined();
    }
    expect((await visit(gate, login(password))).status).toBe(303);
  });

  it("never trusts forwarding headers, a foreign Host or a foreign Origin, even with a session", async () => {
    const gate = lanGate({ hosts: [host] });
    expect((await visit(gate, { headers: { "x-forwarded-for": "127.0.0.1" } })).status).toBe(401);
    const signed = await visit(gate, login(password));
    const cookie = signed.headers["set-cookie"].split(";")[0];
    const foreign: Record<string, string>[] = [{ host: "evil.example:4321" }, { origin: "http://evil.example" }];
    for (const headers of foreign) {
      expect((await visit(gate, { headers: { cookie, ...headers } })).status).toBe(403);
    }
    const crossSite: Record<string, string>[] = [{}, { origin: `http://${host}`, "sec-fetch-site": "cross-site" }];
    for (const headers of crossSite) {
      expect((await visit(gate, { ...login(password), headers })).status).toBe(403);
    }
    expect((await visit(gate, { ...login(password), body: "x".repeat(1025) })).status).toBe(413);
  });

  it("refuses a null Origin rather than weakening the sign-in boundary", async () => {
    const gate = lanGate({ hosts: [host] });
    const rejected = await visit(gate, { ...login(password), headers: { origin: "null" } });
    expect(rejected.status).toBe(403);
    expect(rejected.headers["set-cookie"]).toBeUndefined();
    expect((await visit(gate, { headers: { origin: "null" } })).status).toBe(403);
  });

  it("preserves password-free loopback access but still refuses a foreign Origin", async () => {
    const gate = lanGate({ hosts: [host] });
    for (const ip of ["127.0.0.1", "::ffff:127.0.0.1", "::1"]) {
      expect((await visit(gate, { ip })).admitted).toBe(true);
      expect((await visit(gate, { ip, headers: { origin: "http://evil.example" } })).status).toBe(403);
    }
  });

  it("checks a real HTTP socket without trusting client headers", async () => {
    let gate = lanGate({ hosts: [host] });
    const server = createServer(async (req, res) => {
      if (await gate(req, res)) res.end("loopback admitted");
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("No TCP listener");
      gate = lanGate({ hosts: [`127.0.0.1:${address.port}`] });
      const response = await fetch(`http://127.0.0.1:${address.port}/`);
      expect(await response.text()).toBe("loopback admitted");
    } finally {
      await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    }
  });

  it("keeps a session across a gateway restart, in an owner-only file of hashed tokens", async () => {
    const dir = home();
    const signed = await visit(gateAt({ home: dir, hosts: [host] }), login(password));
    const cookie = signed.headers["set-cookie"].split(";")[0];
    const token = cookie.split("=")[1];
    const file = join(dir, "ui-lan-sessions");
    expect(statSync(file).mode & 0o777).toBe(0o600);
    expect(readFileSync(file, "utf8")).not.toContain(token);
    const restarted = gateAt({ home: dir, hosts: [host, "192.168.1.10:9999"] });
    expect((await visit(restarted, { path: "/api/graphs", headers: { cookie } })).admitted).toBe(true);
    // Cookies do not keep ports apart: a view restarted on another port takes the same browser.
    expect((await visit(restarted, { headers: { cookie, host: "192.168.1.10:9999" } })).admitted).toBe(true);
    // A second sign-in keeps the first.
    const second = await visit(restarted, { ...login(password), ip: "192.168.1.30" });
    expect(second.status).toBe(303);
    expect((await visit(restarted, { headers: { cookie } })).admitted).toBe(true);
  });

  it("refuses a session file that is not owner-only, failing closed", async () => {
    const dir = home();
    const gate = gateAt({ home: dir, hosts: [host] });
    const cookie = (await visit(gate, login(password))).headers["set-cookie"].split(";")[0];
    chmodSync(join(dir, "ui-lan-sessions"), 0o644);
    expect((await visit(gateAt({ home: dir, hosts: [host] }), { headers: { cookie } })).status).toBe(401);
  });

  it("refuses an expired session", async () => {
    let time = 1_000;
    const dir = home();
    const gate = gateAt({ home: dir, hosts: [host], now: () => time });
    const cookie = (await visit(gate, login(password))).headers["set-cookie"].split(";")[0];
    time += LIFETIME * 1000 - 1;
    expect((await visit(gate, { headers: { cookie } })).admitted).toBe(true);
    time += 1;
    expect((await visit(gate, { headers: { cookie } })).status).toBe(401);
    expect((await visit(gateAt({ home: dir, hosts: [host], now: () => time }), { headers: { cookie } })).status).toBe(401);
  });

  it("ends every session when the password is rotated, even in a running view and a sign-in racing the rotation", async () => {
    const dir = home();
    const gate = gateAt({ home: dir, hosts: [host] });
    const cookie = (await visit(gate, login(password))).headers["set-cookie"].split(";")[0];
    const before = readFileSync(join(dir, "ui-lan-sessions"), "utf8");
    setPassword(dir, "2222-3333-4444");
    expect((await visit(gate, { headers: { cookie } })).status).toBe(401);
    // A sign-in checked against the old password that writes after the rotation brings nothing back.
    writeFileSync(join(dir, "ui-lan-sessions"), before, { mode: 0o600 });
    expect((await visit(gateAt({ home: dir, hosts: [host] }), { headers: { cookie } })).status).toBe(401);
    expect((await visit(gate, login(password))).status).toBe(403);
    const fresh = await visit(gate, login("2222-3333-4444"));
    expect(fresh.status).toBe(303);
    expect((await visit(gate, { headers: { cookie: fresh.headers["set-cookie"].split(";")[0] } })).admitted).toBe(true);
    expect((await visit(gate, { headers: { cookie } })).status).toBe(401);
  });

  it("answers the view's own fetches with a signed-out code, and the view reloads to the sign-in page", async () => {
    const gate = lanGate({ hosts: [host] });
    const fetched = await visit(gate, { path: "/api/graphs", headers: { "x-umx-view": "1", "sec-fetch-site": "same-origin" } });
    expect(fetched.status).toBe(401);
    expect(fetched.headers["content-type"]).toBe("application/json");
    expect(fetched.response).toBe(SIGNED_OUT);
    expect((await visit(gate, { path: "/" })).response).toContain('type="password"');

    const reload = vi.fn();
    vi.stubGlobal("window", { location: { reload } });
    vi.stubGlobal("fetch", async () => new Response(fetched.response, { status: 401 }));
    await expect(api("/api/graphs")).rejects.toMatchObject({ issue: { code: "ui_lan_signed_out" } });
    expect(reload).toHaveBeenCalledOnce();
  });
});

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { Readable } from "node:stream";
import { describe, expect, it } from "vitest";
import { lanGate, LOGIN } from "../bin/lan.mjs";

const password = "6A3J-DJ8A-YCK3";
const host = "192.168.1.10:4321";

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
  it("requires a password for every remote path and admits a random, host-bound, in-memory session", async () => {
    const gate = lanGate({ password, hosts: [host, "192.168.1.11:4321"] });
    for (const path of ["/", "/api/graphs", "/api/events", "/_next/static/app.js"]) {
      const res = await visit(gate, { path });
      expect(res.status).toBe(401);
      expect(res.response).toContain('type="password"');
      expect(res.headers["cache-control"]).toBe("no-store");
      expect(res.headers["referrer-policy"]).toBe("same-origin");
    }
    const signed = await visit(gate, login(password));
    expect(signed.status).toBe(303);
    expect(signed.headers["set-cookie"]).toMatch(/^umx-lan=[a-f0-9]{64}; HttpOnly; SameSite=Strict; Path=\/$/);
    const cookie = signed.headers["set-cookie"].split(";")[0];
    expect((await visit(gate, { path: "/api/events", headers: { cookie } })).admitted).toBe(true);
    expect((await visit(gate, { headers: { cookie, host: "192.168.1.11:4321" } })).status).toBe(401);
    expect((await visit(lanGate({ password, hosts: [host] }), { headers: { cookie } })).status).toBe(401);
    expect((await visit(gate, { headers: { cookie: "umx-lan=" + "a".repeat(64) } })).status).toBe(401);
  });

  it("refuses wrong passwords and caps concurrent attempts per real IP, then recovers after a minute", async () => {
    let time = 0;
    const gate = lanGate({ password, hosts: [host], now: () => time });
    const results = await Promise.all(Array.from({ length: 6 }, () => visit(gate, login("wrong"))));
    expect(results.map((r) => r.status).sort()).toEqual([403, 403, 403, 403, 403, 429]);
    expect((await visit(gate, login(password))).status).toBe(429);
    expect((await visit(gate, { ...login(password), ip: "192.168.1.21" })).status).toBe(303);
    time = 60_000;
    expect((await visit(gate, login(password))).status).toBe(303);
  });

  it("refuses mismatched password bytes without throwing and accepts only the exact password", async () => {
    const gate = lanGate({ password, hosts: [host] });
    for (const value of ["", `${password}A`, `\u00e9${password.slice(1)}`, `${password.slice(0, -1)}2`]) {
      const rejected = await visit(gate, login(value));
      expect(rejected.status).toBe(403);
      expect(rejected.headers["set-cookie"]).toBeUndefined();
    }
    expect((await visit(gate, login(password))).status).toBe(303);
  });

  it("never trusts forwarding headers, a foreign Host or a foreign Origin, even with a session", async () => {
    const gate = lanGate({ password, hosts: [host] });
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
    const gate = lanGate({ password, hosts: [host] });
    const rejected = await visit(gate, { ...login(password), headers: { origin: "null" } });
    expect(rejected.status).toBe(403);
    expect(rejected.headers["set-cookie"]).toBeUndefined();
    expect((await visit(gate, { headers: { origin: "null" } })).status).toBe(403);
  });

  it("preserves password-free loopback access but still refuses a foreign Origin", async () => {
    const gate = lanGate({ password, hosts: [host] });
    for (const ip of ["127.0.0.1", "::ffff:127.0.0.1", "::1"]) {
      expect((await visit(gate, { ip })).admitted).toBe(true);
      expect((await visit(gate, { ip, headers: { origin: "http://evil.example" } })).status).toBe(403);
    }
  });

  it("checks a real HTTP socket without trusting client headers", async () => {
    let gate = lanGate({ password, hosts: [host] });
    const server = createServer(async (req, res) => {
      if (await gate(req, res)) res.end("loopback admitted");
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("No TCP listener");
      gate = lanGate({ password, hosts: [`127.0.0.1:${address.port}`] });
      const response = await fetch(`http://127.0.0.1:${address.port}/`);
      expect(await response.text()).toBe("loopback admitted");
    } finally {
      await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    }
  });
});

import { randomBytes, timingSafeEqual } from "node:crypto";
import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";
import { networkInterfaces } from "node:os";
import { join } from "node:path";
import { URLSearchParams } from "node:url";

export const LOGIN = "/_umx/lan-login";
const COOKIE = "umx-lan";
const WINDOW = 60_000;
const MAX_CLIENTS = 4096;

export function lanAddresses() {
  return [...new Set(Object.values(networkInterfaces()).flat().filter((a) => a && !a.internal && a.family === "IPv4").map((a) => a.address))];
}

export function readPassword(home) {
  if (!home) throw new Error("UMX_HOME is required for LAN access.");
  const fd = openSync(join(home, "ui-lan-password"), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size !== 15 || (stat.mode & 0o777) !== 0o600 || stat.uid !== process.getuid()) {
      throw new Error("The LAN password must be an owner-only (0600) file belonging to this user.");
    }
    const password = readFileSync(fd, "utf8").trim();
    if (!/^[2-9A-HJKMNP-Z]{4}(?:-[2-9A-HJKMNP-Z]{4}){2}$/.test(password)) throw new Error("The LAN password file is invalid.");
    return password;
  } finally {
    closeSync(fd);
  }
}

const loopback = (ip) => ip?.startsWith("127.") || ip === "::1" || ip?.startsWith("::ffff:127.");

const PAGE = `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>umx LAN sign in</title><body><main><h1>Graph view</h1><p>Enter the LAN password from this machine's owner.</p>
<form action="${LOGIN}" method="post"><label>Password <input name="password" type="password" autocomplete="current-password" maxlength="14" required autofocus></label>
<button type="submit">Open view</button></form><p>This connection uses plain HTTP. Use a trusted network.</p></main></body></html>`;

function answer(res, status, body, extra = {}) {
  res.writeHead(status, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
    "x-content-type-options": "nosniff",
    "referrer-policy": "same-origin",
    ...extra,
  });
  res.end(body);
}

/** Socket addresses only: forwarded headers are never evidence of a local client. */
export function lanGate({ password, hosts, now = Date.now }) {
  const expected = Buffer.from(password);
  const sessions = new Map();
  const failures = new Map();
  return async (req, res) => {
    const host = req.headers.host;
    const origin = req.headers.origin;
    const site = req.headers["sec-fetch-site"];
    if (!hosts.includes(host) || (origin !== undefined && origin !== `http://${host}`)) {
      answer(res, 403, "Refused.");
      return false;
    }
    const ip = req.socket.remoteAddress;
    const path = req.url?.split("?")[0];
    if (path === LOGIN && (req.method !== "POST" || origin !== `http://${host}` || (site !== undefined && site !== "same-origin"))) {
      answer(res, 403, "Refused.");
      return false;
    }
    if (loopback(ip)) return true;
    const token = /(?:^|;\s*)umx-lan=([a-f0-9]{64})(?:;|$)/.exec(req.headers.cookie ?? "")?.[1];
    if (token && sessions.get(token) === host) return true;
    if (path !== LOGIN) {
      answer(res, 401, PAGE);
      return false;
    }
    const time = now();
    for (const [client, entry] of failures) if (time - entry.since >= WINDOW) failures.delete(client);
    const entry = failures.get(ip) ?? { since: time, count: 0 };
    if (entry.count >= 5 || (!failures.has(ip) && failures.size >= MAX_CLIENTS)) {
      answer(res, 429, "Too many attempts. Try again in one minute.", { "retry-after": String(Math.max(1, Math.ceil((WINDOW - (time - entry.since)) / 1000))) });
      return false;
    }
    // Reserve an attempt before reading: concurrent requests share the same limit.
    entry.count++;
    failures.set(ip, entry);
    let body = "";
    try {
      for await (const chunk of req) {
        body += chunk.toString();
        if (Buffer.byteLength(body) > 1024) {
          answer(res, 413, "Request too large.");
          return false;
        }
      }
    } catch {
      answer(res, 400, "Could not read the password.");
      return false;
    }
    const supplied = Buffer.from(new URLSearchParams(body).get("password") ?? "");
    if (supplied.length !== expected.length || !timingSafeEqual(expected, supplied)) {
      answer(res, 403, PAGE.replace("<form", '<p role="alert">Incorrect password.</p><form'));
      return false;
    }
    if (sessions.size >= MAX_CLIENTS) {
      answer(res, 503, "Too many sessions. Restart the view.");
      return false;
    }
    failures.delete(ip);
    const session = randomBytes(32).toString("hex");
    sessions.set(session, host);
    answer(res, 303, "", { location: "/", "set-cookie": `${COOKIE}=${session}; HttpOnly; SameSite=Strict; Path=/` });
    return false;
  };
}

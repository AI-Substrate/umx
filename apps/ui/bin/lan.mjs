import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { closeSync, constants, fstatSync, fsyncSync, openSync, readFileSync, renameSync, rmSync, writeSync } from "node:fs";
import { networkInterfaces } from "node:os";
import { join } from "node:path";
import { URLSearchParams } from "node:url";

export const LOGIN = "/_umx/lan-login";
const COOKIE = "umx-lan";
const PASSWORD = "ui-lan-password";
const SESSIONS = "ui-lan-sessions";
/** Seconds a LAN session lasts, in the cookie's Max-Age and the stored expiry. */
export const LIFETIME = 30 * 24 * 60 * 60;
const WINDOW = 60_000;
const MAX_CLIENTS = 4096;
const MAX_FILE = 1 << 20;
/** What the view's own fetches (which send x-umx-view) get when sign-in is needed: lib/api.ts reloads on it. */
export const SIGNED_OUT = JSON.stringify({ error: { code: "ui_lan_signed_out", message: "Sign in to the LAN view again." } });

export function lanAddresses() {
  return [...new Set(Object.values(networkInterfaces()).flat().filter((a) => a && !a.internal && a.family === "IPv4").map((a) => a.address))];
}

function openOwned(path, maxSize) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  const stat = fstatSync(fd, { bigint: true });
  if (!stat.isFile() || stat.size > BigInt(maxSize) || (stat.mode & 0o777n) !== 0o600n || stat.uid !== BigInt(process.getuid())) {
    closeSync(fd);
    return null;
  }
  return { fd, stat };
}

/** The password and its file's generation: a rotation writes a new file, so the generation changes with it. */
function passwordAndGeneration(home) {
  if (!home) throw new Error("UMX_HOME is required for LAN access.");
  const opened = openOwned(join(home, PASSWORD), 15);
  if (!opened || opened.stat.size !== 15n) {
    if (opened) closeSync(opened.fd);
    throw new Error("The LAN password must be an owner-only (0600) file belonging to this user.");
  }
  try {
    const password = readFileSync(opened.fd, "utf8").trim();
    if (!/^[2-9A-HJKMNP-Z]{4}(?:-[2-9A-HJKMNP-Z]{4}){2}$/.test(password)) throw new Error("The LAN password file is invalid.");
    return { password, generation: generationOf(opened.stat) };
  } finally {
    closeSync(opened.fd);
  }
}

const generationOf = (stat) => `${stat.dev}:${stat.ino}:${stat.mtimeNs}`;

export function readPassword(home) {
  return passwordAndGeneration(home).password;
}

/** The current password file's generation, or null when it is missing or not owner-only (fail closed). */
function currentGeneration(home) {
  try {
    const opened = openOwned(join(home, PASSWORD), 15);
    if (!opened) return null;
    closeSync(opened.fd);
    return generationOf(opened.stat);
  } catch {
    return null;
  }
}

const hash = (token) => createHash("sha256").update(token).digest("hex");
const hostName = (host) => host.replace(/:\d+$/, "");

/** Sessions persist in UMX_HOME as hashed tokens with an expiry, tied to the password file's generation. */
function sessionStore(home) {
  const path = join(home, SESSIONS);
  const empty = { key: null, generation: null, sessions: new Map() };
  let cache = empty;
  function load() {
    let opened;
    try {
      opened = openOwned(path, MAX_FILE);
    } catch {
      cache = empty;
      return cache;
    }
    if (!opened) {
      cache = empty;
      return cache;
    }
    try {
      const key = `${opened.stat.dev}:${opened.stat.ino}:${opened.stat.mtimeNs}:${opened.stat.size}`;
      if (cache.key === key) return cache;
      const data = JSON.parse(readFileSync(opened.fd, "utf8"));
      const sessions = new Map();
      if (data?.version !== 1 || typeof data.generation !== "string" || !Array.isArray(data.sessions)) throw new Error("invalid");
      for (const s of data.sessions.slice(0, MAX_CLIENTS)) {
        if (/^[a-f0-9]{64}$/.test(s?.hash) && typeof s.host === "string" && Number.isSafeInteger(s.expires)) {
          sessions.set(s.hash, { host: s.host, expires: s.expires });
        }
      }
      cache = { key, generation: data.generation, sessions };
    } catch {
      cache = empty;
    } finally {
      closeSync(opened.fd);
    }
    return cache;
  }
  return {
    admits(token, host, time) {
      const generation = currentGeneration(home);
      const stored = load();
      if (!generation || stored.generation !== generation) return false;
      const entry = stored.sessions.get(hash(token));
      return entry !== undefined && entry.host === hostName(host) && entry.expires > time;
    },
    add(token, host, time, generation) {
      const stored = load();
      const sessions = new Map(stored.generation === generation ? stored.sessions : []);
      for (const [key, s] of sessions) if (s.expires <= time) sessions.delete(key);
      while (sessions.size >= MAX_CLIENTS) {
        let oldest = null;
        for (const [key, s] of sessions) if (!oldest || s.expires < sessions.get(oldest).expires) oldest = key;
        sessions.delete(oldest);
      }
      sessions.set(hash(token), { host: hostName(host), expires: time + LIFETIME * 1000 });
      const body = JSON.stringify({
        version: 1,
        generation,
        sessions: [...sessions].map(([h, s]) => ({ hash: h, host: s.host, expires: s.expires })),
      });
      const temporary = join(home, `.${SESSIONS}-${randomBytes(8).toString("hex")}`);
      const fd = openSync(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
      try {
        try {
          writeSync(fd, body);
          fsyncSync(fd);
        } finally {
          closeSync(fd);
        }
        renameSync(temporary, path);
      } catch (error) {
        rmSync(temporary, { force: true });
        throw error;
      }
    },
  };
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
export function lanGate({ home, hosts, now = Date.now }) {
  if (!home) throw new Error("UMX_HOME is required for LAN access.");
  const sessions = sessionStore(home);
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
    const time = now();
    if (token && sessions.admits(token, host, time)) return true;
    if (path !== LOGIN) {
      if (req.headers["x-umx-view"] === "1") answer(res, 401, SIGNED_OUT, { "content-type": "application/json" });
      else answer(res, 401, PAGE);
      return false;
    }
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
    // Read at each sign-in, so a running view takes a rotated password at once.
    let current;
    try {
      current = passwordAndGeneration(home);
    } catch {
      answer(res, 503, "The LAN password is unavailable. Run umx ui lan-password.");
      return false;
    }
    const expected = Buffer.from(current.password);
    const supplied = Buffer.from(new URLSearchParams(body).get("password") ?? "");
    if (supplied.length !== expected.length || !timingSafeEqual(expected, supplied)) {
      answer(res, 403, PAGE.replace("<form", '<p role="alert">Incorrect password.</p><form'));
      return false;
    }
    failures.delete(ip);
    const session = randomBytes(32).toString("hex");
    try {
      sessions.add(session, host, time, current.generation);
    } catch {
      answer(res, 503, "Could not save the session.");
      return false;
    }
    // SameSite=Lax: a link from another app opens the view signed in. API calls still need x-umx-view, the matching
    // Origin and Sec-Fetch-Site (lib/guard.ts), so Lax lets no other site drive the view.
    answer(res, 303, "", { location: "/", "set-cookie": `${COOKIE}=${session}; Max-Age=${LIFETIME}; HttpOnly; SameSite=Lax; Path=/` });
    return false;
  };
}

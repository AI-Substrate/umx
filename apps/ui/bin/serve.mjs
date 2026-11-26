#!/usr/bin/env node
// Starts the view on loopback by default. UMX_UI_LAN=1 adds a socket-level password gate on all IPv4 interfaces.
//   node bin/serve.mjs [--port=<n>]           the built standalone server (pnpm build first); prints the URL
//   node bin/serve.mjs --dev [--port=<n>]     next dev, for local work only; prints the URL
//   node bin/serve.mjs --stdin [--port=<n>]   how `umx ui` runs it (plan 095): see below
// The run's settings live in a process global (lib/guard.ts runSettings), never in process.env, so nothing the
// server starts inherits them.
//
// --stdin: the server prints one ready line, {"ready":true,"port":N} (plus lan_urls in LAN mode), once accepting.
// stdin stays open while umx runs; when it ends (umx stopped, Ctrl-C, kill -9), the server stops the process group
// umx made for it.
// Each stdin line `code <code>` is a launch code from umx (ws009 M1b2, security D5): the server answers one stdout
// line, {"code":"tab"} when a tab asked for events in the last 30 s (the code is dropped; that tab shows the agent's
// raise), else {"code":"armed"} (good once, for 60 s, taken at /api/launch so the link may name graphs).
// `end <why>` (restarted, unreachable): umx is about to stop this server, and every events poll answers ui_ended.
// UMX_UI_IDLE_MS (umx ui --detach): stop the group once no tab has asked for events for that long.
import { createServer, get, request } from "node:http";
import { setInterval } from "node:timers";
import { setTimeout as sleep } from "node:timers/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { upgradeAllowed } from "./upgrade-guard.mjs";
import { answerCode, endView, idleFor, launchState } from "./launch-codes.mjs";
import { lanAddresses, lanGate, readPassword } from "./lan.mjs";

const LOOPBACK = "127.0.0.1";
const READY_WITHIN_MS = 60_000;
const appDir = join(dirname(fileURLToPath(import.meta.url)), "..");

function refuse(message) {
  console.error(`umx-ui: ${message}`);
  process.exit(2);
}

const args = process.argv.slice(2);
let dev = false;
let fromStdin = false;
let port = 0;
const lan = process.env.UMX_UI_LAN === "1";
delete process.env.UMX_UI_LAN;
if (lan && args.includes("--dev")) refuse("LAN access is available only in the built view.");
const addresses = lan ? lanAddresses() : [];
let password;
if (lan) {
  try {
    password = readPassword(process.env.UMX_HOME);
  } catch (error) {
    refuse(error.message);
  }
}
for (const arg of args) {
  if (arg === "--dev") dev = true;
  else if (arg === "--stdin") fromStdin = true;
  else if (/^--port=\d{1,5}$/.test(arg)) port = Number(arg.slice("--port=".length));
  else refuse(`unknown argument "${arg}". Use --dev or --port=<n>; the view binds ${LOOPBACK} and nothing else.`);
}
if (dev && fromStdin) refuse("--dev and --stdin do not go together.");
for (const name of ["UMX_UI_HOST", "HOST"]) {
  const value = process.env[name];
  if (value !== undefined && value !== "" && value !== LOOPBACK) {
    refuse(`${name}=${value} asks to bind elsewhere; the view binds ${LOOPBACK} only. Unset ${name}.`);
  }
}
if (port > 65535) refuse("--port must be 1 to 65535.");
const idleText = process.env.UMX_UI_IDLE_MS;
delete process.env.UMX_UI_IDLE_MS;
const idleMs = idleText === undefined || idleText === "" ? 0 : Number(idleText);
if (!Number.isInteger(idleMs) || idleMs < 0 || idleMs > 1_800_000 || (idleMs > 0 && !fromStdin)) {
  refuse("UMX_UI_IDLE_MS is 1 to 1800000 ms, and only for umx ui.");
}

/** umx put this process at the head of its own group: stop everything in it, workers included. */
function stopGroup() {
  try {
    process.kill(-process.pid, "SIGTERM");
  } catch {
    process.exit(0);
  }
}

/** A line on stdin: `code <code>` arms a launch code, or leaves it to a connected tab; `end <why>` says why umx
 * is about to stop this server, so its tabs can say so. Anything else is ignored. */
function onLine(line) {
  const end = /^end ([a-z]{1,16})$/.exec(line);
  if (end) {
    endView(launchState(), end[1]);
    return;
  }
  const m = /^code ([A-Za-z0-9_-]{43})$/.exec(line);
  if (!m) return;
  const use = answerCode(launchState(), m[1], Date.now());
  if (use) process.stdout.write(`${JSON.stringify({ code: use })}\n`);
}

/** Reads each line on stdin; stdin's end stops this process group. */
function readStdin() {
  let text = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => {
    text += chunk;
    for (let end = text.indexOf("\n"); end >= 0; end = text.indexOf("\n")) {
      onLine(text.slice(0, end));
      text = text.slice(end + 1);
    }
    if (text.length > 256) refuse("a line on stdin is too long.");
  });
  process.stdin.on("end", stopGroup);
  process.stdin.on("error", stopGroup);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, LOOPBACK, () => {
      const { port: found } = probe.address();
      probe.close(() => resolve(found));
    });
  });
}

/** Whether the server answers on 127.0.0.1:<at> (any HTTP status counts: the guard may refuse the probe). */
function accepting(at) {
  return new Promise((resolve) => {
    const req = get({ host: LOOPBACK, port: at, path: "/", headers: { host: `${LOOPBACK}:${at}` } }, (res) => {
      res.resume();
      resolve(true);
    });
    req.setTimeout(2_000, () => req.destroy());
    req.once("error", () => resolve(false));
  });
}

async function whenAccepting(at) {
  const until = Date.now() + READY_WITHIN_MS;
  while (Date.now() < until) {
    if (await accepting(at)) return;
    await sleep(50);
  }
  refuse(`the server did not accept connections on ${LOOPBACK}:${at}.`);
}

launchState(Date.now());
if (idleMs > 0) {
  const every = Math.min(10_000, Math.max(250, Math.floor(idleMs / 10)));
  setInterval(() => {
    if (idleFor(launchState(), Date.now()) > idleMs) stopGroup();
  }, every).unref();
}

if (fromStdin) readStdin();
if (port === 0) port = await freePort();
const lanUrls = addresses.map((ip) => `http://${ip}:${port}/`);
let backendPort = port;
if (lan) {
  do {
    backendPort = await freePort();
  } while (backendPort === port);
}

globalThis[Symbol.for("umx.ui.run")] = Object.freeze({
  UMX_UI_PORT: String(port),
  UMX_UI_DEV: dev ? "1" : "0",
  UMX_UI_LAN_HOSTS: JSON.stringify(addresses.map((ip) => `${ip}:${port}`)),
});
for (const name of ["UMX_UI_PORT", "UMX_UI_DEV", "HOST"]) {
  delete process.env[name];
}
Object.assign(process.env, { HOSTNAME: LOOPBACK, PORT: String(backendPort), NEXT_TELEMETRY_DISABLED: "1" });

async function announce() {
  if (fromStdin) {
    await whenAccepting(port);
    process.stdout.write(`${JSON.stringify({ ready: true, port, ...(lan ? { lan_urls: lanUrls } : {}) })}\n`);
  } else {
    console.log(`\numx ui: open http://${LOOPBACK}:${port}/\n`);
  }
}

if (dev) {
  const { default: next } = await import("next");
  const app = next({ dev: true, dir: appDir, hostname: LOOPBACK, port });
  await app.prepare();
  const handle = app.getRequestHandler();
  const server = createServer((req, res) => handle(req, res));
  const upgrade = app.getUpgradeHandler();
  server.on("upgrade", (req, socket, head) => {
    if (!upgradeAllowed(req.headers, port)) {
      socket.destroy();
      return;
    }
    upgrade(req, socket, head);
  });
  server.listen(port, LOOPBACK, announce);
} else {
  const entry = join(appDir, ".next", "standalone", "server.js");
  if (fromStdin) {
    // Next prints its banner to stdout; umx reads stdout only for the ready line, so send the banner to stderr.
    console.log = (...parts) => console.error(...parts);
  }
  try {
    await import(pathToFileURL(entry).href);
  } catch (error) {
    refuse(`cannot start ${entry} (${error.code ?? error.message}). Run \`pnpm build\` first.`);
  }
  if (lan) {
    await whenAccepting(backendPort);
    const gate = lanGate({ password, hosts: [`${LOOPBACK}:${port}`, `localhost:${port}`, ...addresses.map((ip) => `${ip}:${port}`)] });
    password = undefined;
    const gateway = createServer(async (req, res) => {
      if (!await gate(req, res)) return;
      const upstream = request({ host: LOOPBACK, port: backendPort, path: req.url, method: req.method, headers: req.headers }, (reply) => {
        res.writeHead(reply.statusCode, reply.headers);
        reply.pipe(res);
        reply.on("error", () => res.destroy());
      });
      upstream.on("error", () => {
        if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain", "cache-control": "no-store" });
        res.end("The graph view is unavailable.");
      });
      res.on("close", () => upstream.destroy());
      req.pipe(upstream);
    });
    gateway.on("upgrade", (_req, socket) => socket.destroy());
    gateway.requestTimeout = 15_000;
    gateway.on("error", (error) => refuse(`cannot bind the LAN view (${error.code ?? error.message}).`));
    gateway.listen(port, "0.0.0.0");
    console.error(`umx-ui: LAN URLs: ${lanUrls.join(", ") || "no LAN IPv4 address found"}`);
    console.error(`umx-ui: password file: ${join(process.env.UMX_HOME, "ui-lan-password")}. Run umx ui lan-password to show it.`);
    console.error("umx-ui: WARNING: plain HTTP exposes the password and graph data to network sniffing. Use only a trusted LAN.");
  }
  await announce();
}

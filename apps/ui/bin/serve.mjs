#!/usr/bin/env node
// Starts the view on 127.0.0.1 only, with a per-run token (security note ws:009, B1 and B2).
//   node bin/serve.mjs [--port=<n>]                 the built standalone server (pnpm build first); prints the link
//   node bin/serve.mjs --dev [--port=<n>]           next dev, for local work only; prints the link
//   node bin/serve.mjs --token-stdin [--port=<n>]   how `umx ui` runs it (plan 095): see below
// The run's token, session and cookie name live in a process global (lib/guard.ts runSettings), never in
// process.env, so nothing the server starts inherits them.
//
// --token-stdin: the token is the first line on stdin (never argv or env). The server prints exactly one line,
// {"ready":true,"port":N}, once it accepts connections, and never prints the token or the link. stdin stays open
// while umx runs; when it ends (umx stopped, Ctrl-C, kill -9) the server stops its whole process group, which umx
// made for it.
// After the token, each stdin line `code <code>` is a launch code from umx (ws009 M1b2, security D5): the server
// answers one stdout line, {"code":"tab"} when a tab asked for events in the last 30 s (the code is dropped; that tab
// shows the agent's raise), else {"code":"armed"} (good once, for 60 s, swapped at /api/session like the token).
// `end <why>` (restarted, unreachable): umx is about to stop this server, and every events poll answers ui_ended.
// UMX_UI_IDLE_MS (umx ui --detach): stop the group once no tab has asked for events for that long.
import { randomBytes } from "node:crypto";
import { createServer, get } from "node:http";
import { setInterval } from "node:timers";
import { setTimeout as sleep } from "node:timers/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { upgradeAllowed } from "./upgrade-guard.mjs";
import { answerCode, endView, idleFor, launchState } from "./launch-codes.mjs";

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
for (const arg of args) {
  if (arg === "--dev") dev = true;
  else if (arg === "--token-stdin") fromStdin = true;
  else if (/^--port=\d{1,5}$/.test(arg)) port = Number(arg.slice("--port=".length));
  else refuse(`unknown argument "${arg}". Use --dev or --port=<n>; the view binds ${LOOPBACK} and nothing else.`);
}
if (dev && fromStdin) refuse("--dev and --token-stdin do not go together.");
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

/** A line after the token: `code <code>` arms a launch code, or leaves it to a connected tab; `end <why>` says why umx
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

/** The first line on stdin; then each further line is read, and stdin's end stops this process group. */
function tokenFromStdin() {
  return new Promise((resolve) => {
    let text = "";
    let got = false;
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => {
      text += chunk;
      for (let end = text.indexOf("\n"); end >= 0; end = text.indexOf("\n")) {
        const line = text.slice(0, end);
        text = text.slice(end + 1);
        if (got) onLine(line);
        else {
          got = true;
          resolve(line);
        }
      }
      if (text.length > 256) refuse("a line on stdin is too long.");
    });
    process.stdin.on("end", () => (got ? stopGroup() : refuse("stdin ended before a token line.")));
    process.stdin.on("error", stopGroup);
  });
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

let token;
if (fromStdin) {
  token = await tokenFromStdin();
  if (!/^[A-Za-z0-9_-]{43}$/.test(token)) refuse("the token line on stdin is not 43 base64url characters.");
} else {
  token = randomBytes(32).toString("base64url");
}
if (port === 0) port = await freePort();
const session = randomBytes(32).toString("base64url");
const cookie = `umx_ui_${randomBytes(6).toString("hex")}`;

globalThis[Symbol.for("umx.ui.run")] = Object.freeze({
  UMX_UI_PORT: String(port),
  UMX_UI_TOKEN: token,
  UMX_UI_SESSION: session,
  UMX_UI_COOKIE: cookie,
  UMX_UI_DEV: dev ? "1" : "0",
});
for (const name of ["UMX_UI_TOKEN", "UMX_UI_SESSION", "UMX_UI_COOKIE", "UMX_UI_PORT", "UMX_UI_DEV", "HOST"]) {
  delete process.env[name];
}
Object.assign(process.env, { HOSTNAME: LOOPBACK, PORT: String(port), NEXT_TELEMETRY_DISABLED: "1" });

async function announce() {
  if (fromStdin) {
    await whenAccepting(port);
    process.stdout.write(`${JSON.stringify({ ready: true, port })}\n`);
  } else {
    console.log(`\numx ui: open http://${LOOPBACK}:${port}/#t=${token}\n`);
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
  await announce();
}

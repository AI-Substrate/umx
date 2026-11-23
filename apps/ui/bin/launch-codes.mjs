// Launch codes and tab presence (ws009 M1b2, security D5), shared inside the one server process: bin/serve.mjs arms
// a code it reads on stdin, POST /api/launch takes one (so a launch link may name graphs), and GET /api/events marks a tab as
// connected. Kept in a process global under a symbol, never in process.env.
import { timingSafeEqual } from "node:crypto";

/** How long an armed code stays good. */
export const CODE_LIFE_MS = 60_000;
/** A tab that asked for events this recently counts as connected (ui-expert, M1b2). */
export const TAB_LIVE_MS = 30_000;
/** A code: 43 base64url characters. */
export const CODE = /^[A-Za-z0-9_-]{43}$/;

const KEY = Symbol.for("umx.ui.launch");

/** This process's codes and polls, made on first use. */
export function launchState(now = Date.now()) {
  const g = /** @type {Record<symbol, unknown>} */ (globalThis);
  g[KEY] ??= { codes: new Map(), polls: 0, seen: null, started: now, ended: null, onEnd: [] };
  return /** @type {import("./launch-codes.d.mts").LaunchState} */ (g[KEY]);
}

function prune(state, now) {
  for (const [code, until] of state.codes) if (until <= now) state.codes.delete(code);
}

/** Keeps `code` for CODE_LIFE_MS, for one use. False for anything not made like a code. */
export function armCode(state, code, now) {
  if (typeof code !== "string" || !CODE.test(code)) return false;
  prune(state, now);
  state.codes.set(code, now + CODE_LIFE_MS);
  return true;
}

/**
 * Takes `given` if it is a live code, and deletes it (used once). Every live code is compared in constant time, so a
 * wrong one neither uses up nor reveals another (security, M1b2).
 */
export function takeCode(state, given, now) {
  prune(state, now);
  const g = Buffer.from(typeof given === "string" ? given : "");
  let found = null;
  for (const code of state.codes.keys()) {
    const c = Buffer.from(code);
    if (g.length === c.length && timingSafeEqual(g, c)) found = code;
  }
  if (found === null) return false;
  state.codes.delete(found);
  return true;
}

export function pollBegan(state, now) {
  state.polls += 1;
  state.seen = now;
}

export function pollEnded(state, now) {
  state.polls = Math.max(0, state.polls - 1);
  state.seen = now;
}

/** Whether a tab is connected: a poll in flight, or one ended within TAB_LIVE_MS. */
export function tabLive(state, now) {
  return state.polls > 0 || (state.seen !== null && now - state.seen < TAB_LIVE_MS);
}

/** How long no tab has asked for events (0 while one asks), counted from the start before any did. */
export function idleFor(state, now) {
  return state.polls > 0 ? 0 : now - (state.seen ?? state.started);
}

/** What a tab is told when its view ends, by the word umx sent on stdin (ui-expert N1). */
export const ENDED = {
  restarted: "The graph view ended (umx daemon restarted). Run umx ui --detach to start it again.",
  unreachable: "The graph view ended (the umx daemon stopped answering). Run umx ui --detach to start it again.",
};

/** umx is about to stop this server: every poll, waiting or new, answers why. Unknown words are ignored. */
export function endView(state, why) {
  if (!Object.hasOwn(ENDED, why) || state.ended !== null) return false;
  state.ended = why;
  for (const f of state.onEnd.splice(0)) f(why);
  return true;
}

/** Resolves with the word once the view ends (at once if it has). */
export function whenEnded(state) {
  if (state.ended !== null) return Promise.resolve(state.ended);
  return new Promise((resolve) => state.onEnd.push(resolve));
}

/** The answer to a `code <code>` line on stdin: a connected tab drops it, else it is armed. */
export function answerCode(state, code, now) {
  if (tabLive(state, now)) return "tab";
  return armCode(state, code, now) ? "armed" : null;
}

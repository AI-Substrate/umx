// Request checks run before routing on every request, static assets included (security note ws:009, B1 to B3).
// Pure functions so the rules are unit-tested without a server.

export type GuardConfig = {
  port: number;
  /** The per-run cookie value the browser gets for the token. */
  session: string;
  /** The per-run cookie name. */
  cookieName: string;
  /** The per-run token printed once to the terminal. */
  token: string;
  dev: boolean;
};

export type GuardInput = {
  method: string;
  pathname: string;
  host: string | null;
  origin: string | null;
  secFetchSite: string | null;
  cookie: string | undefined;
};

export type GuardVerdict = { ok: true } | { ok: false; status: 401 | 403 | 503; reason: string };

const LOOPBACK = "127.0.0.1";

/**
 * Where bin/serve.mjs leaves the run's settings: a process global, never process.env, so no process the server starts
 * inherits the token (security, ws009 S2).
 */
export const RUN_KEY = Symbol.for("umx.ui.run");

export function runSettings(): Record<string, string | undefined> {
  const run = (globalThis as Record<symbol, unknown>)[RUN_KEY];
  return (run as Record<string, string | undefined> | undefined) ?? {};
}

/** Reads the per-run settings bin/serve.mjs left (see runSettings), or null when it did not start us. */
export function readGuardConfig(env: Record<string, string | undefined>): GuardConfig | null {
  const port = Number(env.UMX_UI_PORT);
  const { UMX_UI_TOKEN: token, UMX_UI_SESSION: session, UMX_UI_COOKIE: cookieName } = env;
  if (!Number.isInteger(port) || port <= 0 || port > 65535) return null;
  if (!token || token.length < 43 || !session || session.length < 43 || !cookieName) return null;
  return { port, token, session, cookieName, dev: env.UMX_UI_DEV === "1" };
}

export function expectedOrigin(cfg: GuardConfig): string {
  return `http://${LOOPBACK}:${cfg.port}`;
}

/** Compares two strings in time that depends only on the expected length. */
export function safeEqual(given: string, expected: string): boolean {
  let diff = given.length === expected.length ? 0 : 1;
  for (let i = 0; i < expected.length; i++) {
    diff |= expected.charCodeAt(i) ^ (i < given.length ? given.charCodeAt(i) : 0);
  }
  return diff === 0;
}

export function guard(req: GuardInput, cfg: GuardConfig | null): GuardVerdict {
  if (!cfg) return { ok: false, status: 503, reason: "Start the view with `pnpm start` (bin/serve.mjs)." };
  if (req.host !== `${LOOPBACK}:${cfg.port}`) return { ok: false, status: 403, reason: "Wrong Host." };
  if (req.origin !== null) {
    if (req.origin === "null" || req.origin !== expectedOrigin(cfg)) {
      return { ok: false, status: 403, reason: "Foreign Origin." };
    }
  }
  if (req.pathname !== "/api" && !req.pathname.startsWith("/api/")) return { ok: true };

  const safe = req.method === "GET" || req.method === "HEAD";
  if (req.secFetchSite !== null && req.secFetchSite !== "same-origin") {
    return { ok: false, status: 403, reason: "Cross-site request." };
  }
  if (!safe && req.origin === null) return { ok: false, status: 403, reason: "Missing Origin." };
  if (req.pathname === "/api/session" && req.method === "POST") return { ok: true };
  if (req.cookie === undefined || !safeEqual(req.cookie, cfg.session)) {
    return { ok: false, status: 401, reason: "Open the link printed in the terminal." };
  }
  return { ok: true };
}

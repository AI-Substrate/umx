// Request checks run before routing on every request, static assets included. Pure functions so the rules are
// unit-tested without a server.
//
// The view has no token and no cookie: any browser on this machine that opens http://127.0.0.1:<port>/ (or
// localhost) gets it. What keeps it local: the server binds 127.0.0.1 only (bin/serve.mjs); a Host other than this
// port's loopback names is refused (DNS rebinding); and a page on another origin can neither read the API nor make
// the view act (Origin, Sec-Fetch-Site and a header only the view's own fetches send, which no cross-site request can
// carry without a CORS preflight this server never answers). Any process or user on this machine that can reach
// 127.0.0.1 can read the view: the trust boundary is the user account (crates/umx-cli/docs/ui.md).

export type GuardConfig = {
  port: number;
  dev: boolean;
};

export type GuardInput = {
  method: string;
  pathname: string;
  host: string | null;
  origin: string | null;
  secFetchSite: string | null;
  /** The VIEW_HEADER value, or null when absent. */
  view: string | null;
};

export type GuardVerdict = { ok: true } | { ok: false; status: 403 | 503; reason: string };

/** Every API call the view makes sends this header (lib/api.ts). A cross-site page cannot add it without a preflight. */
export const VIEW_HEADER = "x-umx-view";

/**
 * Where bin/serve.mjs leaves the run's settings: a process global, never process.env, so no process the server starts
 * inherits them.
 */
export const RUN_KEY = Symbol.for("umx.ui.run");

export function runSettings(): Record<string, string | undefined> {
  const run = (globalThis as Record<symbol, unknown>)[RUN_KEY];
  return (run as Record<string, string | undefined> | undefined) ?? {};
}

/** Reads the per-run settings bin/serve.mjs left (see runSettings), or null when it did not start us. */
export function readGuardConfig(env: Record<string, string | undefined>): GuardConfig | null {
  const port = Number(env.UMX_UI_PORT);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) return null;
  return { port, dev: env.UMX_UI_DEV === "1" };
}

/** The Host values this view answers: its port on the loopback names, nothing else (DNS rebinding). */
export function allowedHosts(cfg: GuardConfig): string[] {
  return [`127.0.0.1:${cfg.port}`, `localhost:${cfg.port}`];
}

export function guard(req: GuardInput, cfg: GuardConfig | null): GuardVerdict {
  if (!cfg) return { ok: false, status: 503, reason: "Start the view with `pnpm start` (bin/serve.mjs)." };
  if (req.host === null || !allowedHosts(cfg).includes(req.host)) return { ok: false, status: 403, reason: "Refused." };
  if (req.origin !== null && req.origin !== `http://${req.host}`) return { ok: false, status: 403, reason: "Refused." };
  if (req.pathname !== "/api" && !req.pathname.startsWith("/api/")) return { ok: true };

  if (req.secFetchSite !== null && req.secFetchSite !== "same-origin") return { ok: false, status: 403, reason: "Refused." };
  const safe = req.method === "GET" || req.method === "HEAD";
  if (!safe && req.origin === null) return { ok: false, status: 403, reason: "Refused." };
  if (req.view !== "1") return { ok: false, status: 403, reason: "Refused." };
  return { ok: true };
}

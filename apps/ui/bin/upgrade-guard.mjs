// WebSocket upgrades (next dev's HMR) never reach proxy.ts, and browsers apply no same-origin rule to WebSockets,
// so the dev server checks them here first (security verdict, F1). Plain JS: bin/serve.mjs runs without
// a build step.

/**
 * True only for an upgrade addressed to this port on a loopback name (127.0.0.1 or localhost) from a page of that
 * same origin.
 * @param {Record<string, string | string[] | undefined>} headers
 * @param {number} port
 */
export function upgradeAllowed(headers, port) {
  const host = headers.host;
  if (host !== `127.0.0.1:${port}` && host !== `localhost:${port}`) return false;
  return headers.origin === `http://${host}`;
}

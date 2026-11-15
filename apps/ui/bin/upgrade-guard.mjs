// WebSocket upgrades (next dev's HMR) never reach proxy.ts, and browsers apply no same-origin rule to WebSockets,
// so the dev server checks them here first (security verdict, F1). Plain JS: bin/serve.mjs runs without
// a build step.

/**
 * True only for an upgrade addressed to 127.0.0.1:<port> from a page of this same origin.
 * @param {Record<string, string | string[] | undefined>} headers
 * @param {number} port
 */
export function upgradeAllowed(headers, port) {
  const origin = `http://127.0.0.1:${port}`;
  return headers.host === `127.0.0.1:${port}` && headers.origin === origin;
}

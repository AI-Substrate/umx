// CSP from the security note (B6). Next's own scripts carry the per-request nonce; dev relaxes script-src for
// the dev runtime only.
export function contentSecurityPolicy(nonce: string, dev: boolean): string {
  const script = dev ? `'self' 'nonce-${nonce}' 'unsafe-eval'` : `'self' 'nonce-${nonce}'`;
  return [
    "default-src 'self'",
    `script-src ${script}`,
    `style-src 'self' 'nonce-${nonce}'`,
    "img-src 'self' data:",
    "font-src 'self'",
    "connect-src 'self'",
    "worker-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "frame-ancestors 'none'",
  ].join("; ");
}

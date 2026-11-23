import { NextResponse, type NextRequest } from "next/server";
import { contentSecurityPolicy } from "@/lib/csp";
import { guard, readGuardConfig, runSettings, VIEW_HEADER } from "@/lib/guard";

export function proxy(req: NextRequest) {
  const cfg = readGuardConfig(runSettings());
  const verdict = guard(
    {
      method: req.method,
      pathname: req.nextUrl.pathname,
      host: req.headers.get("host"),
      origin: req.headers.get("origin"),
      secFetchSite: req.headers.get("sec-fetch-site"),
      view: req.headers.get(VIEW_HEADER),
    },
    cfg,
  );
  if (!verdict.ok) {
    return new NextResponse(verdict.reason, {
      status: verdict.status,
      headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
    });
  }
  const nonce = btoa(crypto.randomUUID());
  const csp = contentSecurityPolicy(nonce, cfg?.dev ?? false);
  const headers = new Headers(req.headers);
  headers.set("x-nonce", nonce);
  headers.set("content-security-policy", csp);
  const res = NextResponse.next({ request: { headers } });
  res.headers.set("content-security-policy", csp);
  res.headers.set("x-content-type-options", "nosniff");
  res.headers.set("referrer-policy", "no-referrer");
  res.headers.set("cross-origin-opener-policy", "same-origin");
  res.headers.set("cross-origin-resource-policy", "same-origin");
  res.headers.set("x-frame-options", "DENY");
  return res;
}

export const config = { matcher: "/:path*" };

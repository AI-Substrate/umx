import { timingSafeEqual } from "node:crypto";
import { NextResponse, type NextRequest } from "next/server";
import { launchState, takeCode } from "@/bin/launch-codes.mjs";
import { readGuardConfig, runSettings } from "@/lib/guard";
import { readBody } from "@/lib/http";
import { UmxError } from "@/lib/umx";

// Swaps the token from the URL fragment for the per-run HttpOnly cookie (security note B2). POST only, and the
// proxy has already checked Host and Origin, for a token and a code alike. A launch code (ws009 M1b2, security D5)
// is taken the same way: good once, within 60 s of umx arming it; a wrong one uses up no other.
export async function POST(req: NextRequest) {
  const cfg = readGuardConfig(runSettings());
  if (!cfg) return new NextResponse("Not started by bin/serve.mjs.", { status: 503 });
  let token: unknown;
  try {
    token = (JSON.parse(await readBody(req)) as { token?: unknown }).token;
  } catch (e) {
    if (e instanceof UmxError) return new NextResponse(e.message, { status: e.httpStatus });
    token = undefined;
  }
  const given = Buffer.from(typeof token === "string" ? token : "");
  const expected = Buffer.from(cfg.token);
  const same = given.length === expected.length && timingSafeEqual(given, expected);
  if (!same && !takeCode(launchState(), token, Date.now())) {
    return new NextResponse("That link's token is not this run's, or it was used or is over a minute old.", { status: 401 });
  }
  const res = new NextResponse(null, { status: 204 });
  res.cookies.set(cfg.cookieName, cfg.session, { httpOnly: true, sameSite: "strict", path: "/" });
  return res;
}

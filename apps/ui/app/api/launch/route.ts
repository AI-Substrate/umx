import { NextResponse, type NextRequest } from "next/server";
import { launchState, takeCode } from "@/bin/launch-codes.mjs";
import { readBody } from "@/lib/http";
import { UmxError } from "@/lib/umx";

// Takes a launch code from a launch link's fragment (ws009 M1b2, security D5): good once, within 60 s of umx arming
// it; a wrong one uses up no other. 204 says the link came from umx, so the view may open the graphs it names
// (lib/launch.ts); nothing else changes. POST only, and the proxy has already checked Host, Origin, Sec-Fetch-Site and
// the view header.
export async function POST(req: NextRequest) {
  let code: unknown;
  try {
    code = (JSON.parse(await readBody(req)) as { code?: unknown }).code;
  } catch (e) {
    if (e instanceof UmxError) return new NextResponse(e.message, { status: e.httpStatus });
    code = undefined;
  }
  if (!takeCode(launchState(), code, Date.now())) {
    return new NextResponse("That link's launch code was used, is over a minute old, or is not this run's.", { status: 401 });
  }
  return new NextResponse(null, { status: 204 });
}

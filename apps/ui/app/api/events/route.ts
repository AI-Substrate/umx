import { ENDED, type EndWord, launchState, pollBegan, pollEnded, whenEnded } from "@/bin/launch-codes.mjs";
import { HOLD_MS, parseCursor, sharedHub } from "@/lib/events";
import { MAX_OPEN } from "@/lib/graph/keys";
import { errorResponse, ok } from "@/lib/http";
import { alias, UmxError } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** GET ?graph=<alias>[&graph=...]&epoch=&gen=&since=[&live=<alias>:<epoch>:<rev>...]: the agent events and changes
 * for the graphs a tab has open (1 to MAX_OPEN, plan 133), held up to 20 s (plan 114). One poll per tab, however many
 * graphs it shows. The proxy has already checked the cookie, Host, Origin and Sec-Fetch-Site, as for every API route. */
export async function GET(req: Request) {
  // A tab asking counts as connected (ws009 M1b2): an agent's raise then reaches it, not a new tab.
  const presence = launchState();
  pollBegan(presence, Date.now());
  try {
    const params = new URL(req.url).searchParams;
    const graphs = [...new Set(params.getAll("graph").map((g) => alias(g)))];
    if (graphs.length === 0) alias(undefined);
    if (graphs.length > MAX_OPEN) throw new UmxError("ui_bad_graph", `A view follows at most ${MAX_OPEN} graphs.`, null, 400);
    // umx is stopping this view (ui-expert N1): a waiting poll answers at once, so the tab can say why.
    const ended = (why: EndWord) => new UmxError("ui_ended", ENDED[why], "umx ui --detach", 410);
    if (presence.ended !== null) throw ended(presence.ended);
    const hub = await sharedHub();
    const stop = new AbortController();
    req.signal.addEventListener("abort", () => stop.abort(), { once: true });
    const got = await Promise.race([hub.poll(graphs, parseCursor(params), HOLD_MS, stop.signal), whenEnded(presence)]);
    if (typeof got === "string") {
      stop.abort();
      throw ended(got);
    }
    return ok(got);
  } catch (error) {
    return errorResponse(error);
  } finally {
    pollEnded(presence, Date.now());
  }
}

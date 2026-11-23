import { errorResponse, ok, readBody } from "@/lib/http";
import { sharedPublish } from "@/lib/publish";

export const dynamic = "force-dynamic";

/** POST {nodes}: publishes the person's selection in this graph to the daemon's memory, replacing the last; no nodes
 * clears it (plan 117). The proxy has already checked Host, Origin, Sec-Fetch-Site and the view header. */
export async function POST(req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    const raw = await readBody(req);
    const publish = await sharedPublish();
    return ok(await publish(alias, req.headers.get("content-type"), raw));
  } catch (error) {
    return errorResponse(error);
  }
}

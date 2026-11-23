import { errorResponse, ok, readBody } from "@/lib/http";
import { readWanted, sharedRows } from "@/lib/rows";

export const dynamic = "force-dynamic";

/** POST {nodes, edges}: the rows of the ids a change added or edited, for the graph on screen (plan 127). The proxy
 * has already checked Host, Origin, Sec-Fetch-Site and the view header, as for every API route. */
export async function POST(req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    const want = readWanted(req.headers.get("content-type"), await readBody(req));
    const rows = await sharedRows();
    return ok(await rows(alias, want));
  } catch (error) {
    return errorResponse(error);
  }
}

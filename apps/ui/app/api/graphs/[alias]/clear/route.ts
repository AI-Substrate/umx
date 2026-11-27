import { errorResponse, ok } from "@/lib/http";
import { sharedClear } from "@/lib/publish";

export const dynamic = "force-dynamic";

/** POST: the person's "Clear agent marks" for this graph, as `umx ui clear --graph <alias>` (what agents set: highlight,
 * focus, filter and their notes). The proxy has already checked Host, Origin, Sec-Fetch-Site and the view header. */
export async function POST(_req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    const clear = await sharedClear();
    return ok(await clear(alias));
  } catch (error) {
    return errorResponse(error);
  }
}

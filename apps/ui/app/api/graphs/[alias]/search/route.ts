import { errorResponse, ok } from "@/lib/http";
import { searchGraph } from "@/lib/search";
import { umx, UmxError } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** GET ?q=<text>&mode=title|hybrid: `umx node find` or `umx search` on this graph. */
export async function GET(req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    const params = new URL(req.url).searchParams;
    const mode = params.get("mode") ?? "title";
    if (mode !== "title" && mode !== "hybrid") {
      throw new UmxError("ui_bad_mode", "The search mode is title or hybrid.", null, 400);
    }
    return ok(await searchGraph(umx(), alias, mode, params.get("q") ?? undefined));
  } catch (error) {
    return errorResponse(error);
  }
}

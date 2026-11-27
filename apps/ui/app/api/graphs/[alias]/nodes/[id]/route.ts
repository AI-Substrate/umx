import { errorResponse, ok } from "@/lib/http";
import { loadNode } from "@/lib/snapshot";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** `?entities=off`: the node's own edges only (PRD 005 centre-14, centre-15). */
export async function GET(req: Request, ctx: { params: Promise<{ alias: string; id: string }> }) {
  try {
    const { alias, id } = await ctx.params;
    return ok(await loadNode(umx(), alias, id, new URL(req.url).searchParams.get("entities") === "off"));
  } catch (error) {
    return errorResponse(error);
  }
}

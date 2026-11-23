import { errorResponse, ok } from "@/lib/http";
import { loadSpinePlaces } from "@/lib/spine";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** GET: where the graph's declared spine `name` places each node (spine view). */
export async function GET(_req: Request, ctx: { params: Promise<{ alias: string; name: string }> }) {
  try {
    const { alias, name } = await ctx.params;
    return ok(await loadSpinePlaces(umx(), alias, name));
  } catch (error) {
    return errorResponse(error);
  }
}

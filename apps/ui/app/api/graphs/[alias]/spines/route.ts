import { errorResponse, ok } from "@/lib/http";
import { listSpines } from "@/lib/spine";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** GET: the graph's declared spines (spine view). */
export async function GET(_req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    return ok({ spines: await listSpines(umx(), alias) });
  } catch (error) {
    return errorResponse(error);
  }
}

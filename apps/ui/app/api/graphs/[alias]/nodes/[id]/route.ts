import { errorResponse, ok } from "@/lib/http";
import { loadNode } from "@/lib/snapshot";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

export async function GET(_req: Request, ctx: { params: Promise<{ alias: string; id: string }> }) {
  try {
    const { alias, id } = await ctx.params;
    return ok(await loadNode(umx(), alias, id));
  } catch (error) {
    return errorResponse(error);
  }
}

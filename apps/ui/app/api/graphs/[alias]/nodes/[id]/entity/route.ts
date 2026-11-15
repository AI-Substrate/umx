import { loadEntity } from "@/lib/entity-read";
import { errorResponse, ok } from "@/lib/http";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** A node's entity (ws009 M2): `?with=` the other open graphs, comma-separated; `?merged=1` adds the members' records. */
export async function GET(req: Request, ctx: { params: Promise<{ alias: string; id: string }> }) {
  try {
    const { alias, id } = await ctx.params;
    const url = new URL(req.url);
    const open = (url.searchParams.get("with") ?? "").split(",").filter((g) => g !== "");
    return ok(await loadEntity(umx(), alias, id, open, url.searchParams.get("merged") === "1", req.signal));
  } catch (error) {
    return errorResponse(error);
  }
}

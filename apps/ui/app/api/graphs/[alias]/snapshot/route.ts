import { sharedHub } from "@/lib/events";
import { errorResponse, ok } from "@/lib/http";
import { loadSnapshot } from "@/lib/snapshot";
import { alias as checkAlias, umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** How long the snapshot waits for the change feed to know the graph's revision before reading without it. */
const FOLLOW_MS = 2_000;

export async function GET(req: Request, ctx: { params: Promise<{ alias: string }> }) {
  try {
    const { alias } = await ctx.params;
    // The revision first, then the read: the snapshot is at least that revision, so the tab follows changes from it
    // (plan 127). The feed never blocks a read: with no revision, the tab reloads on its first overflow.
    const live = await sharedHub()
      .then((hub) => hub.follow(checkAlias(alias), FOLLOW_MS, req.signal))
      .catch(() => null);
    return ok({ ...(await loadSnapshot(umx(), alias)), live });
  } catch (error) {
    return errorResponse(error);
  }
}

import { errorResponse, ok } from "@/lib/http";
import { followNamed, listGraphs } from "@/lib/snapshot";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** The graph index for the picker. `?follow=a,b`: a launch link's graphs (plan 215); each the index does not list is
 * asked of umx once, and a former alias comes back in `renamed` with its live alias. */
export async function GET(req: Request) {
  try {
    const follow = new URL(req.url).searchParams.get("follow");
    const read = umx();
    const graphs = await listGraphs(read);
    if (follow === null) return ok({ graphs });
    return ok({ graphs, ...(await followNamed(read, graphs, follow.split(","))) });
  } catch (error) {
    return errorResponse(error);
  }
}

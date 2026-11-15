import { errorResponse, ok } from "@/lib/http";
import { loadShadows, shadowCache } from "@/lib/shadows-read";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** Shadow connections over the open graphs (ws009 M1c): `?graphs=a,b`. A request the browser drops stops the read
 * between pages (review B4). Each call's answer is kept by its graphs' vectors_digest (plan 184). */
export async function GET(req: Request) {
  try {
    const url = new URL(req.url);
    const graphs = (url.searchParams.get("graphs") ?? "").split(",").filter((g) => g !== "");
    return ok(await loadShadows(umx(), graphs, req.signal, shadowCache()));
  } catch (error) {
    return errorResponse(error);
  }
}

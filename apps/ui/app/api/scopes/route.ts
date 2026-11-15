import { errorResponse, ok } from "@/lib/http";
import { listScopes } from "@/lib/snapshot";
import { umx } from "@/lib/umx";

export const dynamic = "force-dynamic";

/** GET: the saved scopes the picker offers (plan 133), each with the graphs it opens and its secured members counted. */
export async function GET() {
  try {
    return ok({ scopes: await listScopes(umx()) });
  } catch (error) {
    return errorResponse(error);
  }
}

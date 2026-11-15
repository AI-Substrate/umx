// The server half of publishing the selection (ws009 S7, plan 117): a tab POSTs {nodes} for the graph it has open,
// and the server checks it and sends `ui select` on the daemon socket. The proxy has already checked the cookie, Host,
// Origin and Sec-Fetch-Site, as for every API route; the browser never talks to the daemon (security T3).
import { BODY_MAX } from "@/lib/http";
import { alias, nodeId, UmxError } from "@/lib/umx";
import { MAX_SELECTION } from "@/lib/umx/socket";


const badBody = (why: string) => new UmxError("ui_bad_selection", `The selection was refused: ${why}.`, null, 400);

/** Reads `{"nodes": [<id>...]}`: node ids only, at most MAX_SELECTION, each once (first kept). */
export function readSelection(raw: string): string[] {
  if (raw.length > BODY_MAX) throw badBody("the body is too big");
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    throw badBody("the body is not JSON");
  }
  const nodes = (body as { nodes?: unknown } | null)?.nodes;
  if (!Array.isArray(nodes)) throw badBody("it needs a list of nodes");
  if (nodes.length > MAX_SELECTION) throw badBody(`a selection is at most ${MAX_SELECTION} nodes`);
  return [...new Set(nodes.map((n) => nodeId(typeof n === "string" ? n : undefined)))];
}

export type PublishDeps = {
  /** Refuses a graph the picker would not open (lib/snapshot graphIds), so publishing never loads or opens one. */
  check: (graph: string) => Promise<void>;
  select: (graph: string, nodes: string[]) => Promise<void>;
};

export function createPublish(deps: PublishDeps) {
  return async (graph: string, contentType: string | null, raw: string): Promise<{ graph: string; selected: number }> => {
    const g = alias(graph);
    if (!/^application\/json(;|$)/i.test(contentType ?? "")) {
      throw new UmxError("ui_bad_selection", "The selection was refused: send it as application/json.", null, 415);
    }
    const nodes = readSelection(raw);
    await deps.check(g);
    await deps.select(g, nodes);
    return { graph: g, selected: nodes.length };
  };
}

const PUBLISH = Symbol.for("umx.ui.publish");

/** The process-wide publisher, on the real daemon. */
export function sharedPublish(): Promise<ReturnType<typeof createPublish>> {
  return import("@/lib/events").then(({ cachedCheck, processWide }) =>
    processWide(PUBLISH, async () => {
      const [{ umxSocket }, { graphIds }, { umx }] = await Promise.all([import("@/lib/umx/socket"), import("@/lib/snapshot"), import("@/lib/umx")]);
      return createPublish({
        check: cachedCheck(async (graph) => void (await graphIds(umx(), graph)), () => Date.now()),
        select: (graph, nodes) => umxSocket().select(graph, nodes),
      });
    }),
  );
}

// A node's entity as umx shows it (ws009 M2, plan 165), for the node panel. Members and proposed partners come from
// `umx entity show` over exactly the open graphs, and are kept to them; members in graphs that are not open, from the
// same read over every loaded open graph. A secured graph is covered only when named, and the view names only
// the open ones, so a secured graph that is not open is never counted or named. Each member's keys and title, and each
// merged record, have the values umx flags as possible secrets replaced here from the flags of the same reply (F188;
// ws009 M2b, lib/secret-flags), so a flagged value never reaches the browser.
import { MAX_OPEN } from "@/lib/graph/keys";
import { alias as checkAlias, type createUmx, nodeId, UmxError } from "@/lib/umx";
import { listGraphs } from "@/lib/snapshot";
import { projected, redactNode } from "@/lib/secret-flags";

type Reader = ReturnType<typeof createUmx>;

/** How a member joined, with no key: the node asked about, a shared key, or a same_as link stored in `graph`. */
export type EntityMatch = { by: "asked" } | { by: "key" } | { by: "link"; graph: string };
/** `keys`: the member's stored keys as umx shows them, a flagged one as its plain scheme and "(hidden key)". */
export type EntityMember = { graph: string; id: string; type: string; title: string; keys: string[]; matched: EntityMatch[] };
export type EntityProposed = { graph: string; id: string; type: string; title: string; with: { graph: string; id: string }; edgeGraph: string };
/** A member's record (`umx node get`), for the merged panel: its keys left out, each flagged value replaced. */
export type EntityRecord = { graph: string; id: string; node: Record<string, unknown>; titleHidden?: boolean };
export type EntityReply = {
  members: EntityMember[];
  proposed: EntityProposed[];
  /** Members in loaded graphs that are not open and not secured, counted per graph. */
  notOpen: { graph: string; count: number }[];
  records?: EntityRecord[];
  /** Members whose records umx refused to read (gone since, or unreadable); the merged record says so. */
  recordsLeftOut?: number;
};

/** Member records read at once. */
const RECORDS_AT_ONCE = 6;

type Row = Record<string, unknown>;
const str = (v: unknown) => (typeof v === "string" ? v : "");

/** `umx:<alias>/<id>` as its alias and id. */
export function addressParts(address: unknown): { graph: string; id: string } {
  const m = /^umx:([^/]+)\/(.+)$/.exec(str(address));
  return m ? { graph: m[1], id: m[2] } : { graph: "", id: "" };
}

function matchOf(m: Row): EntityMatch | null {
  if (m.by === "asked") return { by: "asked" };
  if (m.by === "entity_key") return { by: "key" };
  if (m.by === "same_as") return { by: "link", graph: str(m.graph) };
  return null;
}

/** umx's answers for one member it will not read now (gone since the entity read, or its graph unloaded); anything
 * else fails the whole read. */
const REFUSED = new Set(["node_not_found", "graph_not_loaded"]);

/** A read the browser dropped stops between batches of member records (S8: no cap, so a big entity reads in many). */
function stopped(signal: AbortSignal | undefined) {
  if (signal?.aborted) throw new UmxError("ui_read_cancelled", "The view no longer wants this read.", null, 499);
}

export async function loadEntity(read: Reader, graph: string, id: string, open: readonly string[], merged: boolean, signal?: AbortSignal): Promise<EntityReply> {
  checkAlias(graph);
  nodeId(id);
  const others = [...new Set(open.map((g) => checkAlias(g)))].filter((g) => g !== graph);
  if (others.length + 1 > MAX_OPEN) throw new UmxError("ui_bad_graph", `A read names at most ${MAX_OPEN} graphs.`, null, 400);
  const covered = new Set([graph, ...others]);
  type Shown = { members?: Row[]; proposed?: Row[] };
  const shown = (await read<Shown>("entity show", { graph, id, with: others })).data;
  // Only the open graphs (review B2): whatever umx covered, nothing from another graph is shown or read.
  const inOpen = (r: Row) => covered.has(str(r.graph));
  // Titles and keys through one fail-closed projection (review N1): an unknown flag hides both.
  const title = (r: Row) => projected(r).title;
  const members: EntityMember[] = (shown.members ?? []).filter(inOpen).map((m) => {
    const { title: shownTitle, keys } = projected(m);
    return {
      graph: str(m.graph),
      id: str(m.id),
      type: str(m.type),
      title: shownTitle,
      keys,
      matched: (Array.isArray(m.matched) ? (m.matched as Row[]) : []).map(matchOf).filter((x): x is EntityMatch => x !== null),
    };
  });
  const proposed: EntityProposed[] = (shown.proposed ?? []).filter(inOpen).map((p) => ({
    graph: str(p.graph),
    id: str(p.id),
    type: str(p.type),
    title: title(p),
    with: addressParts(p.with),
    edgeGraph: str(p.edge_graph),
  }));
  const secured = new Set((await listGraphs(read)).filter((g) => g.secured).map((g) => g.alias));
  const loaded = (await read<Shown>("entity show loaded", { graph, id })).data;
  const counts = new Map<string, number>();
  for (const m of loaded.members ?? []) {
    const g = str(m.graph);
    if (covered.has(g) || secured.has(g) || g === "") continue;
    counts.set(g, (counts.get(g) ?? 0) + 1);
  }
  const reply: EntityReply = { members, proposed, notOpen: [...counts].map(([g, count]) => ({ graph: g, count })) };
  if (merged) {
    // Every member's record (S8, lifting an earlier cap of 100), RECORDS_AT_ONCE at a time; one umx refuses is counted.
    reply.records = [];
    let refused = 0;
    for (let i = 0; i < members.length; i += RECORDS_AT_ONCE) {
      stopped(signal);
      const batch = members.slice(i, i + RECORDS_AT_ONCE);
      const got = await Promise.all(
        batch.map((m) =>
          read<{ node: Row }>("node get", { graph: m.graph, id: m.id }).catch((e: unknown) => {
            if (e instanceof UmxError && REFUSED.has(e.code)) return null;
            throw e;
          }),
        ),
      );
      got.forEach((g, j) => {
        if (!g) {
          refused++;
          return;
        }
        const { node, titleHidden } = redactNode(g.data.node);
        delete node.entity_key;
        reply.records!.push({ graph: batch[j].graph, id: batch[j].id, node, ...(titleHidden ? { titleHidden } : {}) });
      });
    }
    stopped(signal);
    if (refused > 0) reply.recordsLeftOut = refused;
  }
  return reply;
}

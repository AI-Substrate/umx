// The rows a change touched (ws009 S5b, plan 127): the browser posts the node and edge ids a change added or edited
// and gets their rows as the snapshot has them. Nodes are read with umx's secret flags (ws009 M2b, lib/flagged-read: a
// `node get` each for a few, else the graph's `node list`), so a flagged title never leaves the server; edges with the
// id-list verb ("edges by id"), at most IDS_MAX ids a read. An id umx does not return is gone: the browser drops it.
import { flaggedNodes, shownTitle } from "@/lib/flagged-read";
import { keysOf } from "@/lib/graph/entities";
import { MAX_CHANGE_IDS } from "@/lib/live";
import { BODY_MAX } from "@/lib/http";
import type { EdgeRow, HubDeclaration, NodeRow } from "@/lib/types";
import { MAX_OPEN } from "@/lib/graph/keys";
import { ontologyOf } from "@/lib/snapshot";
import { alias, IDS_MAX, nodeId, type createUmx, UmxError } from "@/lib/umx";

type Reader = ReturnType<typeof createUmx>;

/** The most ids of one kind a request names: a change's added and edited lists, each at most MAX_CHANGE_IDS. */
export const ROWS_MAX = 2 * MAX_CHANGE_IDS;
/** The first edge read's size. Each edge id costs a scan of the graph's edges in umx's work budget, so a big graph
 * answers query_too_large; the read is then halved, and the size that worked is kept for that graph. */
export const EDGE_CHUNK = 100;
const BUSY_TRIES = 4;

/** `with`: the other graphs open on the canvas (plan 133); an edge between the graph and one of them is read too. */
export type Wanted = { nodes: string[]; edges: string[]; with?: string[] };
/** `external`: [id, type, from alias, from, to alias, to] for edges with an end in one of the `with` graphs.
 * `symmetric`: the graph's symmetric relations now (plan 150 review B1), since a change may have declared one. */
export type RowsReply = {
  graph: string;
  nodes: NodeRow[];
  edges: EdgeRow[];
  external?: [string, string, string, string, string, string, (string | null)?][];
  symmetric?: string[];
  hubs?: HubDeclaration[];
};

const badBody = (why: string) => new UmxError("ui_bad_rows", `The row request was refused: ${why}.`, null, 400);

/** Reads `{"nodes": [<id>...], "edges": [<id>...], "with": [<alias>...]}`: full ids only, at most ROWS_MAX of each,
 * each once; at most MAX_OPEN - 1 other aliases. */
export function readWanted(contentType: string | null, raw: string): Wanted {
  if (!/^application\/json(;|$)/i.test(contentType ?? "")) {
    throw new UmxError("ui_bad_rows", "The row request was refused: send it as application/json.", null, 415);
  }
  if (raw.length > BODY_MAX) throw badBody("the body is too big");
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    throw badBody("the body is not JSON");
  }
  const list = (v: unknown, what: string) => {
    if (v === undefined) return [];
    if (!Array.isArray(v)) throw badBody(`${what} is not a list`);
    if (v.length > ROWS_MAX) throw badBody(`at most ${ROWS_MAX} ${what}`);
    return [...new Set(v.map((n) => nodeId(typeof n === "string" ? n : undefined)))];
  };
  const b = (body ?? {}) as { nodes?: unknown; edges?: unknown; with?: unknown };
  const out: Wanted = { nodes: list(b.nodes, "nodes"), edges: list(b.edges, "edges") };
  if (b.with !== undefined) {
    if (!Array.isArray(b.with)) throw badBody("with is not a list");
    if (b.with.length > MAX_OPEN - 1) throw badBody(`at most ${MAX_OPEN - 1} other graphs`);
    out.with = [...new Set(b.with.map((g) => alias(typeof g === "string" ? g : undefined)))];
  }
  return out;
}

export type RowsDeps = {
  read: Reader;
  /** Refuses a graph the picker would not open (lib/snapshot graphIds), so a row read never loads one. */
  check: (graph: string) => Promise<void>;
  sleep: (ms: number) => Promise<void>;
};

const str = (v: unknown) => (typeof v === "string" ? v : "");

export function createRows(deps: RowsDeps) {
  /** The edge read size that last fitted the work budget, per graph. */
  const edgeChunk = new Map<string, number>();

  /** A read, tried again a few times when umx is running its two queries already (query_busy). */
  async function query<T>(verb: "edges by id", graph: string, ids: string[], others: string[] = []): Promise<T[]> {
    for (let i = 1; ; i++) {
      try {
        return (await deps.read<{ rows: T[] }>(verb, { graph, ids, ...(others.length > 0 ? { with: others } : {}) })).data.rows;
      } catch (e) {
        if (!(e instanceof UmxError) || e.code !== "query_busy" || i >= BUSY_TRIES) throw e;
        await deps.sleep(50 * i);
      }
    }
  }

  return async (graph: string, want: Wanted): Promise<RowsReply> => {
    const g = alias(graph);
    await deps.check(g);
    // Each other graph is checked as the picker would open it: a row read never loads a graph.
    const others = (want.with ?? []).filter((o) => o !== g);
    for (const o of others) await deps.check(o);
    const open = new Set([g, ...others]);
    // A read naming more graphs scans more edges per id, so the size that fitted is kept per graph list.
    const chunkKey = [g, ...[...others].sort()].join(",");
    const asked = { nodes: new Set(want.nodes), edges: new Set(want.edges) };
    const nodes: NodeRow[] = [];
    const got = await flaggedNodes(deps.read, g, want.nodes);
    for (const id of want.nodes) {
      const r = got.get(id);
      if (r && asked.nodes.delete(id)) nodes.push([id, str(r.type), shownTitle(r, id), str(r.status), str(r.updated), keysOf(r.entity_key)]);
    }
    const edges: EdgeRow[] = [];
    const external: NonNullable<RowsReply["external"]> = [];
    let left = want.edges;
    while (left.length > 0) {
      const size = Math.min(edgeChunk.get(chunkKey) ?? EDGE_CHUNK, IDS_MAX);
      const ids = left.slice(0, size);
      type E = { id: unknown; owner?: unknown; type: unknown; from: unknown; to: unknown; from_graph: unknown; to_graph: unknown; auto_added_by?: unknown };
      let rows: E[];
      try {
        rows = await query<E>("edges by id", g, ids, others);
      } catch (e) {
        if (e instanceof UmxError && e.code === "query_too_large" && ids.length > 1) {
          edgeChunk.set(chunkKey, Math.ceil(ids.length / 2));
          continue;
        }
        throw e;
      }
      left = left.slice(ids.length);
      for (const r of rows) {
        const id = str(r.id);
        // The query reads only edges this graph stores; a row from any other owner is never this graph's edge.
        if (r.owner !== undefined && r.owner !== g) continue;
        const [fg, tg] = [str(r.from_graph), str(r.to_graph)];
        // An edge with both ends in this graph, as the snapshot's edgePlace "local"; one to another open graph is
        // external (plan 133). Anything else is not drawn, and so reads as gone.
        if (fg === g && tg === g) {
          if (asked.edges.delete(id)) {
            const row: EdgeRow = [id, str(r.type), str(r.from), str(r.to)];
            if (typeof r.auto_added_by === "string") row.push(r.auto_added_by);
            edges.push(row);
          }
        } else if (open.has(fg) && open.has(tg) && asked.edges.delete(id)) {
          const row: NonNullable<RowsReply["external"]>[number] = [id, str(r.type), fg, str(r.from), tg, str(r.to)];
          if (typeof r.auto_added_by === "string") row.push(r.auto_added_by);
          external.push(row);
        }
      }
    }
    // Every change's request reads the graph's symmetric relations too, even one with no ids: declaring a relation
    // symmetric (umx ontology add relation r --symmetric) can touch no row at all (review B1).
    const ontology = await ontologyOf(deps.read, g);
    return others.length > 0 ? { graph: g, nodes, edges, external, ...ontology } : { graph: g, nodes, edges, ...ontology };
  };
}

const ROWS = Symbol.for("umx.ui.rows");

/** The process-wide row reader, on the real umx. */
export function sharedRows(): Promise<ReturnType<typeof createRows>> {
  return import("@/lib/events").then(({ cachedCheck, processWide }) =>
    processWide(ROWS, async () => {
      const [{ graphIds }, { umx }] = await Promise.all([import("@/lib/snapshot"), import("@/lib/umx")]);
      return createRows({
        read: umx(),
        check: cachedCheck(async (graph) => void (await graphIds(umx(), graph)), () => Date.now()),
        sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
      });
    }),
  );
}

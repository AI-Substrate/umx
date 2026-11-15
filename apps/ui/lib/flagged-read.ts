// Node reads that carry umx's secret flags (ws009 M2b, plan 176): `node get --columns` for a few ids, else the graph's
// `node list` pages. Every title the server sends comes from one of these, with the flags of the same reply (security
// on M2b): reads that carry no flags (live rows by query, search, neighbours, similar_to) take their titles from here.
import { PAGE, type createUmx, UmxError } from "@/lib/umx";
import { hiddenTitle, projected } from "@/lib/secret-flags";

type Reader = ReturnType<typeof createUmx>;
type Row = Record<string, unknown>;

/** Up to this many ids are read one `node get` each; more read the graph's `node list` pages. */
export const GETS_MAX = 24;
const GETS_AT_ONCE = 6;

const str = (v: unknown) => (typeof v === "string" ? v : "");

/** The nodes of `graph` with these ids, each as one reply row with its own flags (id, type, title, status, updated,
 * entity_key, secret_suspected). An id umx does not find is left out; any other failure is thrown, or with `skip` the
 * node is left out too (titles only: a node left out shows no title). */
export async function flaggedNodes(read: Reader, graph: string, ids: readonly string[], skip = false): Promise<Map<string, Row>> {
  const wanted = [...new Set(ids)];
  const out = new Map<string, Row>();
  if (wanted.length === 0) return out;
  if (wanted.length <= GETS_MAX) {
    for (let i = 0; i < wanted.length; i += GETS_AT_ONCE) {
      const got = await Promise.all(
        wanted.slice(i, i + GETS_AT_ONCE).map(async (id) => {
          try {
            return (await read<{ node: Row }>("node row", { graph, id })).data.node;
          } catch (e) {
            if (skip || (e instanceof UmxError && e.code === "node_not_found")) return null;
            throw e;
          }
        }),
      );
      got.forEach((n, j) => {
        if (n && str(n.id) === wanted[i + j]) out.set(wanted[i + j], n);
      });
    }
    return out;
  }
  const left = new Set(wanted);
  for (let offset = 0; left.size > 0; offset += PAGE) {
    const env = await read<{ rows: Row[]; total: number }>("node list", { graph, offset });
    for (const r of env.data.rows) {
      const id = str(r.id);
      if (left.delete(id)) out.set(id, r);
    }
    if (env.data.rows.length < PAGE || offset + PAGE >= env.data.total) break;
  }
  return out;
}

/** A row's title as the view may show it: "<type> · <short id>" when umx flags it, or flags anything the view does
 * not know (lib/secret-flags projected, review N1). */
export const shownTitle = (row: Row, id: string = str(row.id)) => projected(row, id).title;

/** Titles from a read without flags, replaced by the ones a flagged read gives for the same nodes; a node that read
 * does not return (or a read that fails) shows "<type> · <short id>", never the unvouched title. */
export async function vouchTitles<T extends { graph: string; id: string; type: string; title: string }>(read: Reader, items: T[]): Promise<T[]> {
  const byGraph = new Map<string, string[]>();
  for (const it of items) byGraph.set(it.graph, [...(byGraph.get(it.graph) ?? []), it.id]);
  const rows = new Map<string, Map<string, Row> | null>();
  for (const [graph, ids] of byGraph) {
    try {
      rows.set(graph, await flaggedNodes(read, graph, ids, true));
    } catch {
      rows.set(graph, null);
    }
  }
  return items.map((it) => {
    const r = rows.get(it.graph)?.get(it.id);
    return { ...it, title: r ? shownTitle(r, it.id) : hiddenTitle(it.type, it.id) };
  });
}

// Title find, hybrid search and similar_to for the view, each one umx read (lib/umx). Rows keep umx's order, and
// warnings pass through as given: a search without its meaning leg says so, never silently. None of these reads
// carries secret flags, so each hit's title is taken from a flagged read of the node (ws009 M2b, lib/flagged-read).
import { alias as checkAlias, hintsOf, nodeId, queryText, type createUmx, type Envelope, UmxError, warningsOf } from "@/lib/umx";
import { graphIds, toNeighbour } from "@/lib/snapshot";
import { vouchTitles } from "@/lib/flagged-read";
import type { Hit, SearchMode, SearchResult } from "@/lib/types";

type Reader = ReturnType<typeof createUmx>;
type Row = Record<string, unknown>;
type Rows = { rows: Row[]; word_leg?: unknown; similarity?: unknown };

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const strOrNull = (v: unknown): string | null => (typeof v === "string" ? v : null);

/** The reply must be for the command that ran, with rows: help or any other answer is refused, never read as an
 * empty result (review R199-1). */
function rowsOf(env: Envelope<Rows>, command: string): Row[] {
  if (env.command !== command || !Array.isArray(env.data?.rows)) {
    throw new UmxError("ui_umx_output", `umx answered "${str(env.command).slice(0, 40)}" to ${command}.`);
  }
  return env.data.rows;
}

/** Rows of `umx node find` or `umx search --graph <alias>`, as hits; rows from any other graph are counted, not kept. */
export function toHits(mode: SearchMode, rows: Row[], graph: string): { hits: Hit[]; foreign: number } {
  const hits: Hit[] = [];
  let foreign = 0;
  for (const r of rows) {
    if (mode === "hybrid" && r.graph !== graph) {
      foreign++;
      continue;
    }
    hits.push({
      id: str(r.id),
      type: str(r.type),
      title: str(r.title),
      status: str(r.status),
      score: num(mode === "hybrid" ? r.fused : r.score),
      legs: mode === "hybrid" ? strOrNull(r.legs) : null,
      exact: mode === "hybrid" ? strOrNull(r.exact) : null,
    });
  }
  return { hits, foreign };
}

function similarityNote(meta: unknown): string | null {
  if (typeof meta !== "object" || meta === null) return null;
  const m = meta as Row;
  const floor = num(m.floor);
  return `${str(m.model)} · top ${num(m.k) ?? "?"}${floor !== null ? ` · floor ${floor}` : ""}`;
}

/** Hits with their titles from a flagged read: a flagged one, or one that read cannot vouch for, is
 * "<type> · <short id>". */
async function vouched(read: Reader, graph: string, hits: Hit[]): Promise<Hit[]> {
  const v = await vouchTitles(read, hits.map((h) => ({ ...h, graph })));
  return hits.map((h, i) => ({ ...h, title: v[i].title }));
}

export async function searchGraph(read: Reader, graph: string, mode: SearchMode, query: string | undefined, limit?: number): Promise<SearchResult> {
  checkAlias(graph);
  const text = queryText(query);
  // Only a graph the picker would open: a named read of a graph that did not come back would load it.
  await graphIds(read, graph);
  const command = mode === "title" ? "node find" : "search";
  const env = await read<Rows>(command, { graph, query: text, limit });
  const { hits: raw, foreign } = toHits(mode, rowsOf(env, command), graph);
  const hits = await vouched(read, graph, raw);
  const wordLeg = mode === "hybrid" ? strOrNull(env.data.word_leg) : null;
  return { mode, query: text, hits, warnings: warningsOf(env), note: wordLeg ? `word leg: ${wordLeg}` : null, foreign, hints: hintsOf(env) };
}

/** The node's own similar_to list from the daemon: ids, titles and cosines, never vectors. */
export async function similarTo(read: Reader, graph: string, id: string): Promise<SearchResult> {
  // A bad id or alias is refused before anything runs, and only a graph the picker would open is read.
  checkAlias(graph);
  nodeId(id);
  const { localId, aliasOf } = await graphIds(read, graph);
  const env = await read<Rows>("similar to", { graph, id });
  const rows = rowsOf(env, "neighbours");
  const hits: Hit[] = [];
  let foreign = 0;
  for (const row of rows) {
    const n = toNeighbour(row, localId, aliasOf);
    if (n.kind !== "node" || !n.local) {
      foreign++;
      continue;
    }
    hits.push({ id: n.id, type: n.type, title: n.title, status: n.status, score: num(row.score), legs: null, exact: null });
  }
  return { mode: "similar", query: "", hits: await vouched(read, graph, hits), warnings: warningsOf(env), note: similarityNote(env.data.similarity), foreign, hints: hintsOf(env) };
}

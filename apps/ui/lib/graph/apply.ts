// Live updates on the graph on screen (ws009 S5b, plan 127): rows read for a change are applied to the graphology
// instance in place, never by a rebuild, so sigma, the camera and every node's position stay. New nodes start next to
// their neighbours; lib/graph/layout nudge then settles them a little in the worker.
import { assignCustomTypes, assignRelations, nodeSize, hash } from "@/lib/graph/colors";
import { MultiDirectedGraph } from "graphology";
import { addExternal, edgeAttrs, externalRows, newNode, omittedOf, openByGraphId, type OpenGraphs, type ViewGraph } from "@/lib/graph/build";
import type { Groups } from "@/lib/graph/clusters";
import { homeOf } from "@/lib/graph/entities";
import { aliasOfKey, qid } from "@/lib/graph/keys";
import { shapeIndex } from "@/lib/graph/shapes";
import type { RowsReply, Wanted } from "@/lib/rows";
import type { EdgeRow, NodeRow, Snapshot } from "@/lib/types";

/** What applying did on the canvas, by kind. */
export type Count = { added: number; edited: number; removed: number };
/**
 * An ordinary live change's one row request (plans 127, 133): the ids it touched, naming the other open graphs. It is
 * sent even when the change names no ids, since its reply also brings the graph's symmetric relations, and declaring
 * one symmetric can touch no row (plan 150 review B1).
 */
export function readChange(post: (body: Wanted) => Promise<RowsReply>, want: { nodes: string[]; edges: string[] }, others: readonly string[]): Promise<RowsReply> {
  return post(others.length > 0 ? { ...want, with: [...others] } : want);
}

/** A graph's snapshot after an ordinary live change: its counts moved by what was applied, and its symmetric
 * relations as the change's reply has them (kept when the reply has none). Only that graph's snapshot changes. Its
 * external rows lose every edge the change removed (`gone`, the graph's own ids), so an edge deleted live is never
 * read back from them (ws009 M2 review B1: the entity walk follows the ones not drawn). */
export function snapAfter(snap0: Snapshot, outcome: Pick<Outcome, "nodes" | "edges">, rows: Pick<RowsReply, "symmetric" | "hubs">, gone: readonly string[] = []): Snapshot {
  const drop = new Set(gone);
  return {
    ...snap0,
    ...(snap0.external && drop.size > 0 ? { external: snap0.external.filter(([id]) => !drop.has(id)) } : {}),
    totalNodes: snap0.totalNodes + outcome.nodes.added - outcome.nodes.removed,
    totalEdges: snap0.totalEdges + outcome.edges.added - outcome.edges.removed,
    symmetric: rows.symmetric ?? snap0.symmetric,
    ...(rows.hubs !== undefined ? { hubs: rows.hubs } : {}),
  };
}

export type Outcome = {
  nodes: Count;
  edges: Count;
  /** Node ids in index order (NodeAttrs.index), after the change. */
  ids: string[];
  /** Nodes that were not on the canvas before. */
  fresh: string[];
  /** Nodes taken off the canvas. */
  removed: string[];
  /** Nodes whose row or edges changed (the panel rereads a node it shows that is in here). */
  touched: Set<string>;
};

const zero = (): Count => ({ added: 0, edited: 0, removed: 0 });

/**
 * Makes the given keys current: each row is added or updated, each key in `gone` (and each edge that lost an end) is
 * dropped. Rows and keys are graph-qualified (lib/graph/keys, qualifyRows); a new node is in the graph its key names.
 * A row equal to what is drawn changes nothing. An edge is drawn only when both ends are on the canvas, as
 * buildGraph does. Afterwards the nodes are renumbered 0..n-1 in graph order (the worker's arrays are by index),
 * degrees and sizes follow, and new nodes are placed near their neighbours.
 */
const sameKeys = (a: readonly string[] | undefined, b: readonly string[]) => (a ?? []).length === b.length && b.every((k, i) => a![i] === k);

export function applyRows(graph: ViewGraph, rows: { nodes: NodeRow[]; edges: EdgeRow[] }, gone: { nodes: Iterable<string>; edges: Iterable<string> }): Outcome {
  const out: Outcome = { nodes: zero(), edges: zero(), ids: [], fresh: [], removed: [], touched: new Set() };
  const typesBefore = new Set<string>();
  graph.forEachNode((_n, a) => typesBefore.add(a.kind));
  const relationsBefore = new Set<string>();
  graph.forEachEdge((_e, a) => relationsBefore.add(a.kind));
  for (const [id, kind, title, status, updated, keys] of rows.nodes) {
    if (!graph.hasNode(id)) {
      graph.addNode(id, newNode(aliasOfKey(id), kind, title, status, updated, keys));
      out.fresh.push(id);
      out.nodes.added++;
      out.touched.add(id);
      continue;
    }
    const a = graph.getNodeAttributes(id);
    // A row without them (a snapshot node that is no entity's) leaves the entity fields as they are.
    const sameExtra = (updated === undefined || a.updated === updated) && (keys === undefined || sameKeys(a.keys, keys));
    if (a.kind === kind && a.label === title && a.status === status && sameExtra) continue;
    graph.mergeNodeAttributes(id, { kind, shape: shapeIndex(kind), label: title, status, ...(updated !== undefined ? { updated } : {}), ...(keys !== undefined ? { keys } : {}) });
    out.nodes.edited++;
    out.touched.add(id);
  }
  const dropEdge = (id: string) => {
    const [s, t] = graph.extremities(id);
    out.touched.add(s);
    out.touched.add(t);
    graph.dropEdge(id);
  };
  for (const [id, kind, from, to, origin] of rows.edges) {
    const had = graph.hasEdge(id);
    if (!graph.hasNode(from) || !graph.hasNode(to)) {
      if (had) {
        dropEdge(id);
        out.edges.removed++;
      }
      continue;
    }
    if (had) {
      if (graph.getEdgeAttribute(id, "kind") === kind && graph.source(id) === from && graph.target(id) === to && (graph.getEdgeAttribute(id, "autoAddedBy") ?? null) === (origin ?? null)) continue;
      // A new relation or direction: dropped and added again, never repainted in place (sigma trap, plan 102).
      dropEdge(id);
    }
    graph.addEdgeWithKey(id, from, to, edgeAttrs(kind, origin));
    out.touched.add(from);
    out.touched.add(to);
    if (had) out.edges.edited++;
    else out.edges.added++;
  }
  for (const id of gone.edges) {
    if (!graph.hasEdge(id)) continue;
    dropEdge(id);
    out.edges.removed++;
  }
  for (const id of gone.nodes) {
    if (!graph.hasNode(id)) continue;
    out.edges.removed += graph.degree(id);
    graph.forEachNeighbor(id, (n) => out.touched.add(n));
    graph.dropNode(id);
    out.removed.push(id);
    out.touched.add(id);
    out.nodes.removed++;
  }
  const order = graph.order;
  const types = new Set<string>();
  graph.updateEachNodeAttributes((node, a) => {
    a.index = out.ids.length;
    out.ids.push(node);
    a.degree = graph.degree(node);
    a.size = nodeSize(a.degree, order);
    types.add(a.kind);
    return a;
  });
  if ([...types].some((t) => !typesBefore.has(t))) assignCustomTypes(types);
  const relations = new Set<string>();
  graph.forEachEdge((_e, a) => {
    relations.add(a.kind);
  });
  if ([...relations].some((r) => !relationsBefore.has(r))) assignRelations(relations);
  place(graph, out.fresh);
  return out;
}

/** A graph's rows with every id qualified by its alias, as they are keyed on the canvas. External rows (live reads
 * name ends by alias) are kept only when both ends' graphs are open. */
export function qualifyRows(
  alias: string,
  rows: Pick<RowsReply, "nodes" | "edges" | "external">,
  open: ReadonlySet<string>,
): { nodes: NodeRow[]; edges: EdgeRow[] } {
  const nodes = rows.nodes.map(([id, ...rest]): NodeRow => [qid(alias, id), ...rest]);
  const edges = rows.edges.map(([id, kind, from, to, ...origin]): EdgeRow => [qid(alias, id), kind, qid(alias, from), qid(alias, to), ...origin]);
  for (const [id, kind, fa, from, ta, to, ...origin] of rows.external ?? []) {
    if (open.has(fa) && open.has(ta)) edges.push([qid(alias, id), kind, qid(fa, from), qid(ta, to), ...origin]);
  }
  return { nodes, edges };
}

/**
 * Overflow: one graph's snapshot read again, applied as a change against what is drawn of that graph (the camera,
 * positions and every other graph stay). `open` maps the open graphs' ids to their aliases, for its external edges.
 */
export function applySnapshot(graph: ViewGraph, snap: Snapshot, open: OpenGraphs): Outcome {
  const alias = snap.graph;
  // A snapshot row without keys is a node holding none now: one that lost its keys loses them on the canvas too.
  const nodes0 = snap.nodes.map((r): NodeRow => (r.length >= 6 ? r : [r[0], r[1], r[2], r[3], r[4], []]));
  const rows = qualifyRows(alias, { nodes: nodes0, edges: snap.edges }, new Set());
  // Its external edges go through the same rows, so one whose relation or direction changed is re-added; one whose
  // other graph is not open (or open in several copies) is not drawn. An end in the graph itself stays its own.
  for (const { row } of externalRows(snap, open)) if (row) rows.edges.push(row);
  const nodes = new Set(rows.nodes.map((n) => n[0]));
  const edges = new Set(rows.edges.map((e) => e[0]));
  const goneNodes: string[] = [];
  graph.forEachNode((n, a) => {
    if (a.graph === alias && !nodes.has(n)) goneNodes.push(n);
  });
  const goneEdges: string[] = [];
  graph.forEachEdge((e) => {
    if (aliasOfKey(e) === alias && !edges.has(e)) goneEdges.push(e);
  });
  return applyRows(graph, rows, { nodes: goneNodes, edges: goneEdges });
}

/** The ids a change read: every one asked for and not returned is gone, as are the ids it removed. */
export function goneAfterRead(asked: { nodes: string[]; edges: string[] }, got: { nodes: NodeRow[]; edges: EdgeRow[] }, removed: { nodes: string[]; edges: string[] }) {
  const gotNodes = new Set(got.nodes.map((n) => n[0]));
  const gotEdges = new Set(got.edges.map((e) => e[0]));
  return {
    nodes: [...removed.nodes, ...asked.nodes.filter((id) => !gotNodes.has(id))],
    edges: [...removed.edges, ...asked.edges.filter((id) => !gotEdges.has(id))],
  };
}

/** A small offset that is the same for the same id on every run. */
function jitter(id: string, r: number): [number, number] {
  const h = hash(id);
  const angle = ((h % 3600) / 3600) * Math.PI * 2;
  const dist = r * (0.5 + ((h >>> 12) % 1000) / 2000);
  return [Math.cos(angle) * dist, Math.sin(angle) * dist];
}

/**
 * Starting places for `fresh` nodes: the middle of their neighbours that already had a place, else the middle of the
 * nodes of their type, else the middle of the graph; plus a small offset, so two new nodes never sit on each other.
 */
export function place(graph: ViewGraph, fresh: readonly string[]) {
  if (fresh.length === 0) return;
  const isFresh = new Set(fresh);
  let sx = 0;
  let sy = 0;
  let n = 0;
  let minX = Infinity;
  let maxX = -Infinity;
  const byType = new Map<string, { x: number; y: number; n: number }>();
  // Where nodes are laid out: a merged member (ws009 M2) is drawn at its entity's centroid (review B6).
  graph.forEachNode((id, attrs) => {
    if (isFresh.has(id)) return;
    const a = homeOf(attrs);
    sx += a.x;
    sy += a.y;
    n++;
    minX = Math.min(minX, a.x);
    maxX = Math.max(maxX, a.x);
    const t = byType.get(attrs.kind) ?? { x: 0, y: 0, n: 0 };
    t.x += a.x;
    t.y += a.y;
    t.n++;
    byType.set(attrs.kind, t);
  });
  // About one node's spacing: the graph's width over the nodes along it.
  const step = n > 1 && maxX > minX ? (maxX - minX) / Math.sqrt(n) : 10;
  for (const id of fresh) {
    let x = 0;
    let y = 0;
    let k = 0;
    graph.forEachNeighbor(id, (nb, attrs) => {
      if (isFresh.has(nb)) return;
      const a = homeOf(attrs);
      x += a.x;
      y += a.y;
      k++;
    });
    const t = byType.get(graph.getNodeAttribute(id, "kind"));
    const [cx, cy] = k > 0 ? [x / k, y / k] : t ? [t.x / t.n, t.y / t.n] : n > 0 ? [sx / n, sy / n] : [0, 0];
    const [dx, dy] = jitter(id, step);
    graph.mergeNodeAttributes(id, { x: cx + dx, y: cy + dy });
  }
}

/** A nudge for the worker: the new nodes, their neighbours and the nodes near them, by local index; only new ones move. */
export type NudgeInput = { n: number; edges: Uint32Array; positions: Float32Array; sizes: Float32Array; fixed: Uint8Array };

/** The most nodes a nudge takes: new ones first, then their neighbours, then the nodes around them. */
export const NUDGE_MAX = 4_000;

export function nudgeInput(graph: ViewGraph, fresh: readonly string[]): { ids: string[]; input: NudgeInput } | null {
  const moving = fresh.filter((id) => graph.hasNode(id));
  if (moving.length === 0) return null;
  const ids: string[] = [];
  const local = new Map<string, number>();
  const take = (id: string) => {
    if (local.has(id) || ids.length >= NUDGE_MAX) return;
    local.set(id, ids.length);
    ids.push(id);
  };
  moving.forEach(take);
  for (const id of moving) graph.forEachNeighbor(id, take);
  // The nodes near the new ones push back on them: a grid of cells about two node spacings wide.
  let minX = Infinity;
  let maxX = -Infinity;
  graph.forEachNode((_id, attrs) => {
    const a = homeOf(attrs);
    minX = Math.min(minX, a.x);
    maxX = Math.max(maxX, a.x);
  });
  const cell = Math.max(1e-6, (maxX > minX ? (maxX - minX) / Math.sqrt(graph.order) : 10) * 2);
  const key = (x: number, y: number) => `${Math.floor(x / cell)},${Math.floor(y / cell)}`;
  const wanted = new Set<string>();
  for (const id of moving) {
    const a = homeOf(graph.getNodeAttributes(id));
    const cx = Math.floor(a.x / cell);
    const cy = Math.floor(a.y / cell);
    for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) wanted.add(`${cx + dx},${cy + dy}`);
  }
  graph.forEachNode((id, attrs) => {
    const a = homeOf(attrs);
    if (wanted.has(key(a.x, a.y))) take(id);
  });
  const n = ids.length;
  const positions = new Float32Array(n * 2);
  const sizes = new Float32Array(n);
  const fixed = new Uint8Array(n).fill(1);
  ids.forEach((id, i) => {
    const a = graph.getNodeAttributes(id);
    const home = homeOf(a);
    positions[i * 2] = home.x;
    positions[i * 2 + 1] = home.y;
    sizes[i] = a.size;
  });
  for (const id of moving) fixed[local.get(id)!] = 0;
  const pairs: number[] = [];
  graph.forEachEdge((_e, _a, s, t) => {
    const i = local.get(s);
    const j = local.get(t);
    if (i !== undefined && j !== undefined && i !== j) pairs.push(i, j);
  });
  return { ids, input: { n, edges: Uint32Array.from(pairs), positions, sizes, fixed } };
}

/** Puts a nudge's result back: only the nodes that were free to move, and only those still on the canvas. A merged
 * member (ws009 M2) gets it as where it is laid out; the app merges again after a nudge (review B6). */
export function applyNudge(graph: ViewGraph, ids: readonly string[], fixed: Uint8Array, positions: Float32Array) {
  ids.forEach((id, i) => {
    if (fixed[i] || !graph.hasNode(id)) return;
    const [x, y] = [positions[i * 2], positions[i * 2 + 1]];
    graph.mergeNodeAttributes(id, graph.getNodeAttribute(id, "homeX") !== undefined ? { homeX: x, homeY: y } : { x, y });
  });
}

/**
 * Groups made for one numbering of the nodes (`from`, the ids by index), read for another (`to`): each node keeps its
 * group, a new node is in none until Louvain runs again, and a group whose head left loses its pill. So clusters and
 * hulls stay put between a live change and the next clustering.
 */
export function remapGroups(groups: Groups, from: readonly string[], to: readonly string[]): Groups {
  if (from === to) return groups;
  const at = new Map<string, number>();
  to.forEach((id, i) => at.set(id, i));
  const of = new Int32Array(to.length).fill(-1);
  from.forEach((id, i) => {
    const j = at.get(id);
    if (j !== undefined && i < groups.of.length) of[j] = groups.of[i];
  });
  const sizes = new Array<number>(groups.sizes.length).fill(0);
  for (const g of of) if (g >= 0) sizes[g]++;
  const heads = groups.heads.map((h, rank) => {
    const j = at.get(from[h]);
    if (j !== undefined) return j;
    const k = of.indexOf(rank);
    return k;
  });
  return { of, sizes, heads };
}

/** The live indicator's words for what a change did on the canvas: "+3 nodes, 1 edge removed". Empty for nothing. */
export function changeText(o: { nodes: Count; edges: Count }): string {
  const n = (k: number, one: string) => `${k} ${one}${k === 1 ? "" : "s"}`;
  const parts: string[] = [];
  if (o.nodes.added > 0) parts.push(`+${n(o.nodes.added, "node")}`);
  if (o.edges.added > 0) parts.push(`+${n(o.edges.added, "edge")}`);
  if (o.nodes.edited > 0) parts.push(`${n(o.nodes.edited, "node")} changed`);
  if (o.edges.edited > 0) parts.push(`${n(o.edges.edited, "edge")} changed`);
  if (o.nodes.removed > 0) parts.push(`${n(o.nodes.removed, "node")} removed`);
  if (o.edges.removed > 0) parts.push(`${n(o.edges.removed, "edge")} removed`);
  return parts.join(", ");
}

/** The extent sigma frames a graph by (every node's x and y, as sigma's own graphExtent). */
export type Extent = { x: [number, number]; y: [number, number] };
export function extentOf(graph: ViewGraph): Extent {
  if (graph.order === 0) return { x: [0, 1], y: [0, 1] };
  const e: Extent = { x: [Infinity, -Infinity], y: [Infinity, -Infinity] };
  graph.forEachNode((_n, a) => {
    e.x = [Math.min(e.x[0], a.x), Math.max(e.x[1], a.x)];
    e.y = [Math.min(e.y[0], a.y), Math.max(e.y[1], a.y)];
  });
  return e;
}

/**
 * Another open set from what is on screen (ws009 M1b1): a copy of `prev` (so the old canvas is never touched) without
 * the graphs not in `target`, plus each `added` graph's snapshot, plus the kept graphs' external edges into the added
 * ones. What a kept graph shows stays exactly as it is, live changes and held revisions included; nothing is read
 * again for it. `kept` and `added` hold one snapshot per graph in `target` between them.
 */
export function reopen(prev: ViewGraph, target: readonly string[], kept: readonly Snapshot[], added: readonly Snapshot[]) {
  const graph: ViewGraph = new MultiDirectedGraph();
  prev.forEachNode((n, a) => {
    if (target.includes(a.graph)) graph.addNode(n, { ...a });
  });
  prev.forEachEdge((e, a, s, t) => {
    if (graph.hasNode(s) && graph.hasNode(t) && target.includes(aliasOfKey(e))) graph.addEdgeWithKey(e, s, t, { ...a });
  });
  const byAlias = new Map([...kept, ...added].map((sn) => [sn.graph, sn]));
  const snaps = target.map((a) => byAlias.get(a)!);
  const open = openByGraphId(snaps);
  const fresh: string[] = [];
  for (const sn of added) fresh.push(...applySnapshot(graph, sn, open).fresh);
  for (const sn of kept) addExternal(graph, sn, open);
  // Renumbered, degrees and sizes once more, now that every edge is in.
  const { ids } = applyRows(graph, { nodes: [], edges: [] }, { nodes: [], edges: [] });
  const omitted = new Map(snaps.map((sn) => [sn.graph, omittedOf(graph, sn, open)]));
  return { graph, ids, snaps, open, omitted, fresh };
}

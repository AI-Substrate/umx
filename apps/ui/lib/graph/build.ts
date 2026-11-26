import { MultiDirectedGraph } from "graphology";
import { convexHull, type Groups, type IndexGraph } from "@/lib/graph/clusters";
import {
  assignCustomTypes,
  assignRelations,
  groupColor,
  nodeSize,
  PROVENANCE_TYPES,
  RETIRED_STATUSES,
  statusColor,
  statusLook,
  type Theme,
  typeColor,
} from "@/lib/graph/colors";
import { type Filters, NO_FILTERS, nodeShown, toggled, type Visibility } from "@/lib/graph/filters";
import { graphColor } from "@/lib/graph/graph-colors";
import { qid } from "@/lib/graph/keys";
import { shapeIndex } from "@/lib/graph/shapes";
import { inPolygon } from "@/lib/selection";
import type { EdgeRow, Neighbour, Snapshot } from "@/lib/types";

export type NodeAttrs = {
  label: string;
  kind: string;
  status: string;
  /** The alias of the graph the node is in (plan 133); its key is `<graph>/<id>` (lib/graph/keys). */
  graph: string;
  x: number;
  y: number;
  size: number;
  degree: number;
  /** Collision-free radius in layout units, projected to pixels by the canvas. */
  clearance?: number;
  hub?: boolean;
  color: string;
  borderColor: string;
  borderSize: number;
  /** The node program's shape number for its type (lib/graph/shapes shapeIndex), set wherever `kind` is. */
  shape: number;
  hidden: boolean;
  index: number;
  /** Group rank (0 is the biggest) for hulls and colour by cluster; -1 when in none. */
  group: number;
  /** When it was last updated, and its entity keys (ws009 M2): read only where entities need them. Keys never reach
   * a label, callout, tooltip, title or anything shared. */
  updated?: string;
  keys?: string[];
  /** While entities are merged (ws009 M2), where the node is laid out; x and y then hold its entity's centroid, as sigma
   * draws a node only where its attributes put it. */
  homeX?: number;
  homeY?: number;
};
export type EdgeAttrs = { kind: string; color: string; size: number; hidden: boolean };
export type ViewGraph = MultiDirectedGraph<NodeAttrs, EdgeAttrs>;

export type { Visibility };
export type ColourBy = "type" | "cluster" | "status" | "graph";

export function newNode(graph: string, kind: string, title: string, status: string, updated?: string, keys?: string[]): NodeAttrs {
  return {
    ...(updated !== undefined ? { updated } : {}),
    ...(keys !== undefined && keys.length > 0 ? { keys } : {}),
    label: title,
    kind,
    status,
    graph,
    x: 0,
    y: 0,
    size: 4,
    degree: 0,
    color: "#888888",
    borderColor: "#444444",
    borderSize: 0.15,
    shape: shapeIndex(kind),
    hidden: false,
    index: 0,
    group: -1,
  };
}

export const edgeAttrs = (kind: string): EdgeAttrs => ({ kind, color: "#999999", size: kind === "part_of" ? 0.6 : 1, hidden: false });

/** Why an edge a graph stores is not on the canvas (plan 133): an end in a graph that is not open (or not loaded),
 * or an end umx could not resolve, or one in a graph open in several copies (never guessed). Keyed by edge key. */
export type Omitted = Map<string, "notOpen" | "broken">;

/** The links between open graphs drawn, and the edges left off by why (review N6 and B7). */
export type ExternalCount = { drawn: number; notOpen: number; broken: number };

/** The open graphs by graph id: how an external end finds its alias on the canvas. A graph id open in several copies
 * maps to null: which copy an end means is never guessed (review B1). */
export type OpenGraphs = ReadonlyMap<string, string | null>;

export function openByGraphId(snaps: readonly Pick<Snapshot, "graph" | "graphId">[]): Map<string, string | null> {
  const out = new Map<string, string | null>();
  for (const s of snaps) out.set(s.graphId, out.has(s.graphId) && out.get(s.graphId) !== s.graph ? null : s.graph);
  return out;
}

/** The canvas key of an external row's end: an end in the storing graph itself is that graph's own node (umx resolves
 * it locally, so a copy's own end stays in the copy); any other end, in the one open graph with its id. Null when its
 * graph is not open or is open in several copies. */
function endKey(graphId: string, node: string, owner: { alias: string; graphId: string }, open: OpenGraphs): string | null {
  const alias = graphId === owner.graphId ? owner.alias : open.get(graphId);
  return alias ? qid(alias, node) : null;
}

/**
 * Where the node panel's neighbours are on the canvas (plan 133, review B1). A neighbour in the read graph
 * itself (`local`) is in the selected node's own graph, `owner`, even when a copy of that graph is open too; one in
 * another graph is in the one open graph with its id; one whose graph is not open, or open in several copies, is not
 * on the canvas. A secured graph that is not open is never named (d-20).
 */
export function neighbourLinks(open: OpenGraphs, owner: string | null, secured: ReadonlySet<string>) {
  const aliasOf = (n: Neighbour) => (n.local && owner ? owner : open.get(n.graph));
  return {
    target: (n: Neighbour): string | null => {
      const a = aliasOf(n);
      return n.kind === "node" && a ? qid(a, n.id) : null;
    },
    graphName: (n: Neighbour): string => {
      const a = aliasOf(n);
      if (a) return a;
      if (a === null) return "(open in several copies)";
      return secured.has(n.graph) ? "(secured)" : (n.graphAlias ?? n.graph);
    },
  };
}

/** A snapshot's external rows as canvas edge rows, each end resolved (endKey), or null for one that cannot be drawn. */
export function externalRows(snap: Pick<Snapshot, "graph" | "graphId" | "external">, open: OpenGraphs): { key: string; row: EdgeRow | null; ambiguous: boolean }[] {
  const owner = { alias: snap.graph, graphId: snap.graphId };
  return (snap.external ?? []).map(([id, kind, fromGraph, from, toGraph, to]) => {
    const [s, t] = [endKey(fromGraph, from, owner, open), endKey(toGraph, to, owner, open)];
    const ambiguous = open.get(fromGraph) === null || open.get(toGraph) === null;
    return { key: qid(snap.graph, id), row: s && t ? [qid(snap.graph, id), kind, s, t] : null, ambiguous };
  });
}

/** Adds a snapshot's external edges whose two ends are both on the canvas. An edge already drawn is left as it is. */
export function addExternal(graph: ViewGraph, snap: Pick<Snapshot, "graph" | "graphId" | "external">, open: OpenGraphs) {
  for (const { row } of externalRows(snap, open)) {
    if (!row) continue;
    const [key, kind, s, t] = row;
    if (graph.hasEdge(key) || !graph.hasNode(s) || !graph.hasNode(t)) continue;
    graph.addEdgeWithKey(key, s, t, edgeAttrs(kind));
  }
}

/** What a snapshot's graph stores that is not on the canvas: its external edges not drawn, and its broken edges. */
export function omittedOf(graph: ViewGraph, snap: Pick<Snapshot, "graph" | "graphId" | "external" | "broken">, open: OpenGraphs): Omitted {
  const out: Omitted = new Map();
  for (const { key, ambiguous } of externalRows(snap, open)) if (!graph.hasEdge(key)) out.set(key, ambiguous ? "broken" : "notOpen");
  for (const id of snap.broken ?? []) out.set(qid(snap.graph, id), "broken");
  return out;
}

/** The links drawn between open graphs (an edge whose ends are in two graphs), and what each graph left off. */
export function externalCount(graph: ViewGraph, omitted: Iterable<Omitted>): ExternalCount {
  const out: ExternalCount = { drawn: 0, notOpen: 0, broken: 0 };
  graph.forEachEdge((_e, _a, _s, _t, sa, ta) => {
    if (sa.graph !== ta.graph) out.drawn++;
  });
  for (const om of omitted) for (const why of om.values()) out[why]++;
  return out;
}

/**
 * A live change's effect on what a graph left off: an edge removed is gone; an edge read again is either drawn now,
 * or still off (for the reason it had, else its other graph is not open).
 */
export function omittedAfter(prev: Omitted | undefined, graph: ViewGraph, read: readonly string[], removed: readonly string[]): Omitted {
  const out: Omitted = new Map(prev);
  for (const key of removed) out.delete(key);
  for (const key of read) {
    if (graph.hasEdge(key)) out.delete(key);
    else out.set(key, out.get(key) ?? "notOpen");
  }
  return out;
}

/**
 * Builds the view graph from one snapshot per open graph, keyed `<alias>/<id>` so equal ids in two graphs never
 * collide. Each graph's own edges, then its external edges whose ends are both open. Positions stay at 0 until the
 * layout worker sends its seed.
 */
export function buildGraph(input: Snapshot | readonly Snapshot[]): { graph: ViewGraph; ids: string[]; droppedEdges: number; external: ExternalCount; omitted: Map<string, Omitted> } {
  const snaps: readonly Snapshot[] = Array.isArray(input) ? input : [input as Snapshot];
  const graph: ViewGraph = new MultiDirectedGraph();
  const ids: string[] = [];
  for (const snap of snaps) {
    for (const [id, kind, title, status, updated, keys] of snap.nodes) {
      const key = qid(snap.graph, id);
      if (graph.hasNode(key)) continue;
      graph.addNode(key, { ...newNode(snap.graph, kind, title, status, updated, keys), index: ids.length });
      ids.push(key);
    }
  }
  let droppedEdges = 0;
  for (const snap of snaps) {
    for (const [id, kind, from, to] of snap.edges) {
      const [key, s, t] = [qid(snap.graph, id), qid(snap.graph, from), qid(snap.graph, to)];
      if (!graph.hasNode(s) || !graph.hasNode(t) || graph.hasEdge(key)) {
        droppedEdges++;
        continue;
      }
      // Colour comes from the canvas palette in the edge reducer; part_of is drawn thinner (design note section 4).
      graph.addEdgeWithKey(key, s, t, edgeAttrs(kind));
    }
  }
  const open = openByGraphId(snaps);
  for (const snap of snaps) addExternal(graph, snap, open);
  const omitted = new Map(snaps.map((snap) => [snap.graph, omittedOf(graph, snap, open)]));
  const order = graph.order;
  const types = new Set<string>();
  graph.forEachNode((node, a) => {
    a.degree = graph.degree(node);
    a.size = nodeSize(a.degree, order);
    types.add(a.kind);
  });
  assignCustomTypes(types);
  const relations = new Set<string>();
  graph.forEachEdge((_e, a) => {
    relations.add(a.kind);
  });
  assignRelations(relations);
  return { graph, ids, droppedEdges, external: externalCount(graph, omitted.values()), omitted };
}

/** The graph by node index (insertion order, which is NodeAttrs.index), for the worker and the grouping. With
 * `ranks` (alias -> rank, more than one graph open), `part` holds each node's graph rank, so the layout seeds each
 * graph in a region of its own. */
export function indexGraph(graph: ViewGraph, ranks?: ReadonlyMap<string, number>): IndexGraph {
  const edges = new Uint32Array(graph.size * 2);
  const partOf = new Uint8Array(graph.size);
  const similarity = new Float32Array(graph.size);
  let e = 0;
  graph.forEachEdge((_k, a, _s, _t, sa, ta) => {
    edges[e * 2] = sa.index;
    edges[e * 2 + 1] = ta.index;
    partOf[e] = a.kind === "part_of" ? 1 : 0;
    similarity[e] = a.kind === "similar_to" ? 1 : 0;
    e++;
  });
  if (!ranks || ranks.size < 2) return { n: graph.order, edges, partOf, similarity };
  const part = new Int32Array(graph.order);
  graph.forEachNode((_n, a) => {
    part[a.index] = ranks.get(a.graph) ?? 0;
  });
  return { n: graph.order, edges, partOf, part, similarity };
}

/** Groups by graph (plan 133): one group per open graph, ranked in the order the graphs are open, so a graph keeps
 * its rank (and colour) as the others change size. Hidden nodes are in none. Heads are each graph's best connected
 * shown node; the hull's label is the alias, not the head's title. */
export function graphGroups(graph: ViewGraph, ranks: ReadonlyMap<string, number>, mask: Uint8Array): Groups {
  const of = new Int32Array(graph.order).fill(-1);
  const sizes = new Array<number>(ranks.size).fill(0);
  const heads = new Array<number>(ranks.size).fill(-1);
  const best = new Array<number>(ranks.size).fill(-1);
  graph.forEachNode((_n, a) => {
    const r = ranks.get(a.graph);
    if (r === undefined || !mask[a.index]) return;
    of[a.index] = r;
    sizes[r]++;
    if (a.degree > best[r]) {
      best[r] = a.degree;
      heads[r] = a.index;
    }
  });
  return { of, sizes, heads };
}

/** 1 for each node (by index) the toggles and filters show. */
export function shownMask(graph: ViewGraph, visible: Visibility, filters: Filters = NO_FILTERS): Uint8Array {
  const out = new Uint8Array(graph.order);
  graph.forEachNode((_n, a) => {
    out[a.index] = nodeShown(a, visible, filters) ? 1 : 0;
  });
  return out;
}

/** 1 for each edge (in indexGraph's order) whose relation is not filtered out. Hidden ends are the node mask's job. */
export function edgeMask(graph: ViewGraph, filters: Filters): Uint8Array {
  const out = new Uint8Array(graph.size);
  let e = 0;
  graph.forEachEdge((_k, a) => {
    out[e++] = filters.relations.has(a.kind) ? 0 : 1;
  });
  return out;
}

export function applyVisibility(graph: ViewGraph, visible: Visibility, filters: Filters = NO_FILTERS) {
  // No paint-only hint: sigma would repaint in place and throw if it has not processed the graph yet.
  graph.updateEachNodeAttributes((_node, a) => {
    a.hidden = !nodeShown(a, visible, filters);
    return a;
  });
}

/** Whether an edge is drawn: both ends shown (after applyVisibility) and its relation not filtered out. */
export function edgeShown(graph: ViewGraph, edge: string, filters: Filters): boolean {
  const [s, t] = graph.extremities(edge);
  return !filters.relations.has(graph.getEdgeAttribute(edge, "kind")) && !graph.getNodeAttribute(s, "hidden") && !graph.getNodeAttribute(t, "hidden");
}

export type ChipCounts = { types: Map<string, number>; statuses: Map<string, number>; relations: Map<string, number> };

/**
 * Chip counts: how many nodes or edges a chip stands for, on or off. A type's count is its nodes the toggles and the
 * status chips let through; a status's count, its nodes the toggles and the type chips let through; a relation's,
 * its edges with both ends shown. So turning a chip off takes exactly its count off the canvas, and the counts of
 * the chips that are on add up to what is drawn.
 */
export function chipCounts(graph: ViewGraph, visible: Visibility, filters: Filters): ChipCounts {
  const types = new Map<string, number>();
  const statuses = new Map<string, number>();
  const relations = new Map<string, number>();
  const shown = new Map<string, boolean>();
  graph.forEachNode((id, a) => {
    const ok = toggled(a, visible);
    if (ok && !filters.statuses.has(a.status)) types.set(a.kind, (types.get(a.kind) ?? 0) + 1);
    if (ok && !filters.types.has(a.kind)) statuses.set(a.status, (statuses.get(a.status) ?? 0) + 1);
    shown.set(id, nodeShown(a, visible, filters));
  });
  graph.forEachEdge((_e, a, s, t) => {
    if (shown.get(s) && shown.get(t)) relations.set(a.kind, (relations.get(a.kind) ?? 0) + 1);
  });
  return { types, statuses, relations };
}

/** Chips to list for one dimension: every key with a count, plus any turned off, biggest first then by name. */
export function chipList(count: Map<string, number>, off: ReadonlySet<string>): [string, number][] {
  const keys = new Set([...count.keys(), ...off]);
  return [...keys].map((k): [string, number] => [k, count.get(k) ?? 0]).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

/** Sets node colour, ring and group for the theme, the colour mode and the groups. Cheap enough to rerun on every
 * change. Edges are styled by the canvas's edge reducer, never mutated here: an edge update sigma takes as
 * paint-only throws when it lands before sigma has processed a just-mounted graph. */
export function applyLook(graph: ViewGraph, theme: Theme, by: ColourBy, groups: Groups | null, ranks?: ReadonlyMap<string, number>) {
  graph.updateEachNodeAttributes((_node, a) => {
    const look = statusLook(a.status, theme);
    a.group = groups?.of[a.index] ?? -1;
    a.color =
      by === "cluster"
        ? groupColor(a.group, a.group >= 0 ? groups!.sizes[a.group] : 0, theme, look.muted)
        : by === "status"
          ? statusColor(look.kind, theme)
          : by === "graph"
            ? graphColor(ranks?.get(a.graph) ?? 0, theme, look.muted)
            : typeColor(a.kind, theme, look.muted);
    a.borderColor = look.ring;
    a.borderSize = look.ringSize;
    return a;
  });
}

/**
 * How much the open graphs overlap on the canvas (plan 133 review N1): the share of shown nodes that sit inside the
 * convex hull of another open graph's shown nodes. 0 is fully apart; one graph gives 0.
 */
export function graphOverlap(graph: ViewGraph): number {
  const points = new Map<string, number[]>();
  graph.forEachNode((_n, a) => {
    if (a.hidden) return;
    const list = points.get(a.graph);
    if (list) list.push(a.x, a.y);
    else points.set(a.graph, [a.x, a.y]);
  });
  if (points.size < 2) return 0;
  const hulls = [...points].map(([g, list]) => [g, convexHull(list)] as const);
  let inside = 0;
  let total = 0;
  graph.forEachNode((_n, a) => {
    if (a.hidden) return;
    total++;
    if (hulls.some(([g, h]) => g !== a.graph && h.length >= 6 && inPolygon(h, a.x, a.y))) inside++;
  });
  return total === 0 ? 0 : inside / total;
}

/** Legend counts. Types and statuses count only the nodes on the canvas, so a hidden provenance type is not
 * listed (ui-expert N5); provenance and retired count every node, for their toggles. */
export function counts(graph: ViewGraph) {
  const types = new Map<string, number>();
  const statuses = new Map<string, number>();
  const graphs = new Map<string, number>();
  let provenance = 0;
  let retired = 0;
  graph.forEachNode((_n, a) => {
    if (!a.hidden) {
      types.set(a.kind, (types.get(a.kind) ?? 0) + 1);
      const kind = statusLook(a.status, "light").kind;
      statuses.set(kind, (statuses.get(kind) ?? 0) + 1);
      graphs.set(a.graph, (graphs.get(a.graph) ?? 0) + 1);
    }
    if (PROVENANCE_TYPES.has(a.kind)) provenance++;
    if (RETIRED_STATUSES.has(a.status)) retired++;
  });
  return { types, statuses, graphs, provenance, retired };
}

// Grouping for hulls and colour by cluster (design note section 1). Louvain runs in the layout worker on the
// visible subgraph only: provenance and retired nodes out, edges undirected, part_of weighted up, seeded so the
// same graph clusters the same way. Parent groups by the part_of tree instead. Pure, so tests run it directly.
import { UndirectedGraph } from "graphology";
import louvain from "graphology-communities-louvain";
import { rng } from "@/lib/graph/rng";

export const PART_OF_WEIGHT = 3;

/** The graph by node index, as the worker gets it: edges as source, target pairs, partOf[e] = 1 for part_of. `part`,
 * when several graphs are open, is each node's graph rank (plan 133): the seed gives each graph a region. */
export type IndexGraph = {
  n: number; edges: Uint32Array; partOf: Uint8Array; part?: Int32Array; similarity?: Float32Array;
  /** 1 = declared hub, 0 = authoritative non-hub, -1 = older daemon's part_of preview. */
  declared?: Int8Array;
  membership?: Uint8Array;
};

/** of[i] is node i's group rank (0 is the biggest) or -1 when it is hidden; heads[r] names group r. */
export type Groups = { of: Int32Array; sizes: number[]; heads: number[] };

/** Renumbers raw group ids (-1 = none) by size, biggest first, ties to the group with the smaller first member. */
function rank(raw: Int32Array, score: (i: number) => number): Groups {
  const members = new Map<number, number[]>();
  raw.forEach((g, i) => {
    if (g < 0) return;
    const list = members.get(g);
    if (list) list.push(i);
    else members.set(g, [i]);
  });
  const order = [...members.values()].sort((a, b) => b.length - a.length || a[0] - b[0]);
  const of = new Int32Array(raw.length).fill(-1);
  const sizes: number[] = [];
  const heads: number[] = [];
  order.forEach((list, r) => {
    let head = list[0];
    for (const i of list) {
      of[i] = r;
      if (score(i) > score(head)) head = i;
    }
    sizes.push(list.length);
    heads.push(head);
  });
  return { of, sizes, heads };
}

/** `edges`, when given, is 1 for each edge whose relation is not filtered out (build.ts edgeMask); the rest are left out. */
export function louvainGroups(g: IndexGraph, visible: Uint8Array, resolution: number, seed: number, edges?: Uint8Array): Groups {
  const graph = new UndirectedGraph<Record<string, never>, { weight: number }>();
  const degree = new Uint32Array(g.n);
  for (let i = 0; i < g.n; i++) if (visible[i]) graph.addNode(i);
  for (let e = 0; e < g.edges.length / 2; e++) {
    const s = g.edges[e * 2];
    const t = g.edges[e * 2 + 1];
    if (s === t || !visible[s] || !visible[t] || (edges && !edges[e])) continue;
    degree[s]++;
    degree[t]++;
    const w = g.partOf[e] ? PART_OF_WEIGHT : 1;
    const key = graph.edge(s, t);
    if (key === undefined) graph.addEdge(s, t, { weight: w });
    else graph.updateEdgeAttribute(key, "weight", (old) => (old ?? 0) + w);
  }
  const raw = new Int32Array(g.n).fill(-1);
  if (graph.order > 0) {
    const found = louvain(graph, { getEdgeWeight: "weight", resolution, rng: rng(seed) });
    for (const [node, community] of Object.entries(found)) raw[Number(node)] = community;
  }
  return rank(raw, (i) => degree[i]);
}

/** Groups by the part_of tree: each visible node joins the root it reaches by following its first part_of edge
 * (in edge order) to a visible parent. A node with no part_of is a group of one. */
export function parentGroups(g: IndexGraph, visible: Uint8Array, edges?: Uint8Array): Groups {
  const parent = new Int32Array(g.n).fill(-1);
  for (let e = 0; e < g.edges.length / 2; e++) {
    const s = g.edges[e * 2];
    const t = g.edges[e * 2 + 1];
    if (g.partOf[e] && s !== t && parent[s] < 0 && visible[s] && visible[t] && (!edges || edges[e])) parent[s] = t;
  }
  const root = new Int32Array(g.n).fill(-1);
  for (let i = 0; i < g.n; i++) {
    if (!visible[i]) continue;
    const path: number[] = [];
    const seen = new Set<number>();
    let at = i;
    while (root[at] < 0 && parent[at] >= 0 && !seen.has(at)) {
      seen.add(at);
      path.push(at);
      at = parent[at];
    }
    const top = root[at] >= 0 ? root[at] : at;
    root[at] = top;
    for (const p of path) root[p] = top;
  }
  return rank(root, (i) => (root[i] === i ? 1 : 0));
}

/** A screen box [x0, y0, x1, y1]. */
export type Box = [number, number, number, number];

/** Which cluster pills to draw (ui-expert N10): in the order given (biggest group first), a pill is skipped when its
 * box overlaps one already placed, the rule sigma uses for node labels. */
export function placePills(boxes: Box[]): boolean[] {
  const placed: Box[] = [];
  return boxes.map((box) => {
    if (placed.some((b) => b[0] < box[2] && box[0] < b[2] && b[1] < box[3] && box[1] < b[3])) return false;
    placed.push(box);
    return true;
  });
}

/** Convex hull (Andrew's monotone chain) of flat [x0, y0, x1, y1, ...] points, counter-clockwise, flat. */
export function convexHull(points: number[]): number[] {
  const n = points.length / 2;
  if (n < 3) return points.slice();
  const idx = Array.from({ length: n }, (_, i) => i).sort(
    (a, b) => points[a * 2] - points[b * 2] || points[a * 2 + 1] - points[b * 2 + 1],
  );
  const cross = (o: number, a: number, b: number) =>
    (points[a * 2] - points[o * 2]) * (points[b * 2 + 1] - points[o * 2 + 1]) -
    (points[a * 2 + 1] - points[o * 2 + 1]) * (points[b * 2] - points[o * 2]);
  const lower: number[] = [];
  for (const p of idx) {
    while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop();
    lower.push(p);
  }
  const upper: number[] = [];
  for (let k = idx.length - 1; k >= 0; k--) {
    const p = idx[k];
    while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop();
    upper.push(p);
  }
  const hull = lower.slice(0, -1).concat(upper.slice(0, -1));
  return hull.flatMap((p) => [points[p * 2], points[p * 2 + 1]]);
}

import { UndirectedGraph } from "graphology";
import louvain from "graphology-communities-louvain";
import type { IndexGraph } from "@/lib/graph/clusters";
import { separateDiscs, clearanceRadius } from "@/lib/graph/clearance";
import { rng } from "@/lib/graph/rng";

export type LayoutMode = "normal" | "degree" | "similarity" | "declared";
export type DegreeMode = "incoming" | "outgoing" | "connections";
export type HubOptions = { mode: LayoutMode; degree: DegreeMode; count: number };
export const HUB_DEFAULTS: HubOptions = { mode: "similarity", degree: "connections", count: 10 };
export function hubCount(value: number): number {
  return Number.isInteger(value) && value >= 1 && value <= 100 ? value : HUB_DEFAULTS.count;
}
export const LAYOUT_MODES = [["normal", "Normal"], ["degree", "Degree hubs"], ["similarity", "Similarity hubs"], ["declared", "Declared hubs"]] as const;
export const DEGREE_MODES = [["incoming", "Most incoming"], ["outgoing", "Most outgoing"], ["connections", "Most connections"]] as const;
export type SimilarityLink = { a: string; b: string; score: number };
export type HubPlan = { centres: number[]; owner: Int32Array; fallback?: boolean };
const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;

/** Degree is by distinct neighbour, not number of parallel edges. A local maximum wins ties by graph-qualified id. */
export function findHubs(g: IndexGraph, ids: readonly string[], options: HubOptions, resolution: number, seed: number, pairs: readonly SimilarityLink[] = []): HubPlan {
  const owner = new Int32Array(g.n).fill(-1);
  if (options.mode === "normal") return { centres: [], owner };
  const incoming = Array.from({ length: g.n }, () => new Set<number>());
  const outgoing = Array.from({ length: g.n }, () => new Set<number>());
  const adjacent = Array.from({ length: g.n }, () => new Set<number>());
  const similarity = new UndirectedGraph<Record<string, never>, { weight: number }>();
  const link = (a: number, b: number, weight: number, served = false) => {
    if (a === b || !Number.isFinite(weight) || weight <= 0) return;
    similarity.mergeNode(a);
    similarity.mergeNode(b);
    const edge = similarity.edge(a, b);
    if (edge === undefined) similarity.addEdge(a, b, { weight });
    else similarity.updateEdgeAttribute(edge, "weight", (was) => served ? weight : Math.max(was ?? 0, weight));
  };
  for (let e = 0; e < g.edges.length / 2; e++) {
    const a = g.edges[e * 2], b = g.edges[e * 2 + 1];
    if (a === b) continue;
    if (options.mode !== "declared" || g.partOf[e]) {
      outgoing[a].add(b);
      incoming[b].add(a);
      adjacent[a].add(b);
      adjacent[b].add(a);
    }
    if (g.similarity?.[e]) link(a, b, g.similarity[e]);
  }
  const index = new Map(ids.map((id, i) => [id, i]));
  for (const p of [...pairs].sort((a, b) => compare(a.a, b.a) || compare(a.b, b.b) || a.score - b.score)) {
    const a = index.get(p.a), b = index.get(p.b);
    if (a !== undefined && b !== undefined) link(a, b, p.score, true);
  }
  const central = new Float64Array(g.n);
  const found = similarity.order > 0 ? louvain(similarity, { resolution, getEdgeWeight: "weight", rng: rng(seed) }) : {};
  if (similarity.order > 0) {
    similarity.forEachEdge((_e, a, s, t) => {
      if (found[s] === found[t]) { central[Number(s)] += a.weight; central[Number(t)] += a.weight; }
    });
  }
  if (options.mode === "similarity") {
    if (similarity.order === 0) return { ...findHubs(g, ids, { ...options, mode: "degree", degree: "connections" }, resolution, seed), fallback: true };
    const clusters = new Map<number, number[]>();
    for (const node of similarity.nodes()) {
      const i = Number(node), group = found[node];
      const list = clusters.get(group);
      if (list) list.push(i);
      else clusters.set(group, [i]);
    }
    const score = central;
    const centres: number[] = [];
    for (const members of clusters.values()) {
      if (members.length < 2) continue;
      members.sort((a, b) => score[b] - score[a] || compare(ids[a], ids[b]));
      const head = members[0];
      centres.push(head);
      for (const i of members) owner[i] = head;
    }
    centres.sort((a, b) => score[b] - score[a] || compare(ids[a], ids[b]));
    const chosen = centres.slice(0, hubCount(options.count));
    const kept = new Set(chosen);
    owner.forEach((head, i) => { if (!kept.has(head)) owner[i] = -1; });
    return { centres: chosen, owner };
  }
  const neighbours = options.mode === "declared" || options.degree === "incoming" ? incoming : options.degree === "outgoing" ? outgoing : adjacent;
  const score = neighbours.map((set) => set.size);
  const maximum = central.reduce((a, b) => Math.max(a, b), 0);
  const rank = (i: number) => score[i] * (1 + (maximum > 0 ? 0.25 * central[i] / maximum : 0));
  const centres = Array.from({ length: g.n }, (_, i) => i)
    .filter((i) => score[i] >= 2)
    .sort((a, b) => rank(b) - rank(a) || score[b] - score[a] || compare(ids[a], ids[b]))
    .slice(0, hubCount(options.count));
  for (const h of centres) owner[h] = h;
  for (const h of centres) for (const i of options.mode === "declared" ? incoming[h] : adjacent[h]) if (owner[i] < 0) owner[i] = h;
  return { centres, owner };
}

/** Fans are packed as whole discs so clearing a crowded hub never destroys its rings. Nodes outside any fan
 * retain their force positions as preferences, not an assignment to an arbitrary centre. */
export function fanLayout(positions: Float32Array, sizes: Float32Array, plan: HubPlan, spacing: number): Float32Array {
  const out = positions.slice();
  const members = new Map<number, number[]>();
  plan.owner.forEach((head, i) => {
    if (head < 0 || head === i) return;
    const list = members.get(head);
    if (list) list.push(i);
    else members.set(head, [i]);
  });
  const bodies: { x: number; y: number; radius: number; nodes: number[]; head: number }[] = [];
  for (const head of plan.centres) {
    const list = members.get(head) ?? [];
    const maximum = list.reduce((max, i) => Math.max(max, clearanceRadius(sizes[i], spacing)), clearanceRadius(sizes[head], spacing));
    const pitch = maximum * 2.1;
    let offset = 0, ring = 1;
    for (; offset < list.length; ring++) {
      const radius = ring * pitch;
      const count = Math.min(list.length - offset, Math.max(1, Math.floor(Math.PI / Math.asin(Math.min(1, maximum / radius)))));
      for (let j = 0; j < count; j++) {
        const i = list[offset + j], angle = j * Math.PI * 2 / count;
        out[i * 2] = positions[head * 2] + Math.cos(angle) * radius;
        out[i * 2 + 1] = positions[head * 2 + 1] + Math.sin(angle) * radius;
      }
      offset += count;
    }
    bodies.push({ x: positions[head * 2], y: positions[head * 2 + 1], radius: Math.max(maximum, (ring - 1) * pitch + maximum), nodes: [head, ...list], head });
  }
  for (let i = 0; i < sizes.length; i++) if (plan.owner[i] < 0) bodies.push({ x: out[i * 2], y: out[i * 2 + 1], radius: clearanceRadius(sizes[i], spacing), nodes: [i], head: i });
  separateDiscs(bodies).forEach((p, b) => {
    for (const i of bodies[b].nodes) {
      out[i * 2] += p.x - bodies[b].x;
      out[i * 2 + 1] += p.y - bodies[b].y;
    }
  });
  return out;
}

/** Sort and compact visible input once, at the worker boundary. Force, clustering and hub tie-breaks then all see
 * canonical node/edge order, regardless of snapshot insertion order. */
export function canonicalGraph(g: IndexGraph, ids: readonly string[], visible: Uint8Array, edges: Uint8Array) {
  const order = Array.from({ length: g.n }, (_, i) => i).filter((i) => visible[i]).sort((a, b) => compare(ids[a], ids[b]));
  const at = new Int32Array(g.n).fill(-1);
  order.forEach((i, j) => { at[i] = j; });
  const rows: { a: number; b: number; part: number; similarity: number }[] = [];
  for (let e = 0; e < g.edges.length / 2; e++) {
    const a = at[g.edges[e * 2]], b = at[g.edges[e * 2 + 1]];
    if (edges[e] && a >= 0 && b >= 0 && a !== b) rows.push({ a, b, part: g.partOf[e], similarity: g.similarity?.[e] ?? 0 });
  }
  rows.sort((a, b) => a.a - b.a || a.b - b.b || a.part - b.part || a.similarity - b.similarity);
  const parts = g.part ? [...new Set(order.map((i) => g.part![i]))].sort((a, b) => {
    const first = (p: number) => ids[order.find((i) => g.part![i] === p)!];
    return compare(first(a), first(b));
  }) : [];
  const graph: IndexGraph = {
    n: order.length,
    edges: Uint32Array.from(rows.flatMap((e) => [e.a, e.b])),
    partOf: Uint8Array.from(rows.map((e) => e.part)),
    similarity: Float32Array.from(rows.map((e) => e.similarity)),
    ...(g.part ? { part: Int32Array.from(order.map((i) => parts.indexOf(g.part![i]))) } : {}),
  };
  return { graph, order, ids: order.map((i) => ids[i]) };
}

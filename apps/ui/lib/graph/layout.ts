// Layout, pure so tests can run it (the worker only wires messages to it). Seed: a circle pack by group, so each
// cluster starts as a disc on a sunflower spiral. Then ForceAtlas2 with Barnes-Hut above 2k nodes, then noverlap.
// It stops on an iteration count, with the time budget only as a safety cap, so the same graph lays out the same
// way on any machine (ui-expert N3). A run that hits the cap says so (`capped`), and is then not reproducible.
import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import noverlap from "graphology-layout-noverlap";
import type { Groups, IndexGraph } from "@/lib/graph/clusters";
import { rng } from "@/lib/graph/rng";
import { clearanceRadius, separateDiscs } from "@/lib/graph/clearance";
import { SPACING_LINEAR } from "@/lib/graph/spacing";

export type LayoutResult = { positions: Float32Array; iterations: number; ms: number; capped: boolean };

export function iterationsFor(n: number): number {
  return n <= 500 ? 900 : n <= 3000 ? 600 : 300;
}

/** What a layout is a function of (ui-expert N3): the graph, its seed and the spacing. The same graph with the same
 * seed and spacing lays out the same way. */
export type LayoutSeed = { seed: number; spacing: number };

/** How far apart the seed starts nodes, against plan 133's first seed (design section 3); the ring of several graphs
 * is derived from it, so it widens too. */
export const SEED_SPREAD = 1.5;

/** Each node's group, a hidden node taking the group of its first visible neighbour; -1 for the rest. */
function homeGroups(g: IndexGraph, groups: Groups): Int32Array {
  const group = Int32Array.from(groups.of);
  for (let e = 0; e < g.edges.length / 2; e++) {
    const s = g.edges[e * 2];
    const t = g.edges[e * 2 + 1];
    if (group[s] < 0 && groups.of[t] >= 0) group[s] = groups.of[t];
    if (group[t] < 0 && groups.of[s] >= 0) group[t] = groups.of[s];
  }
  return group;
}

/** Positions (x0, y0, x1, y1, ...) packed by group. A hidden node sits with the group of its first visible
 * neighbour, else with the other loners, so turning it on later shows it near its friends. */
export function seedPositions(g: IndexGraph, groups: Groups, seed: number): Float32Array {
  const random = rng(seed);
  const rest = groups.sizes.length;
  const group = homeGroups(g, groups);
  // Several graphs open (plan 133): each graph's groups are packed in a region of their own, the regions on a ring,
  // so a graph starts apart from the others and only their external edges pull them together.
  const parts = g.part ? g.part.reduce((m, v) => Math.max(m, v), 0) + 1 : 1;
  const partOf = (i: number) => (g.part ? g.part[i] : 0);
  const members: number[][][] = Array.from({ length: parts }, () => Array.from({ length: rest + 1 }, () => []));
  group.forEach((r, i) => members[partOf(i)][r < 0 ? rest : r].push(i));
  const positions = new Float32Array(g.n * 2);
  const golden = Math.PI * (3 - Math.sqrt(5));
  const sizes = members.map((lists) => lists.reduce((n, l) => n + l.length, 0));
  const spreads = sizes.map((n) => Math.sqrt(n) * 6 * SEED_SPREAD);
  // Each graph's seed reaches its last group's spiral slot plus that disc; the ring keeps neighbouring graphs apart.
  const reach = members.map((lists, p) => {
    const slots = lists.filter((l) => l.length > 0);
    const biggest = slots.reduce((m, l) => Math.max(m, l.length), 0);
    return Math.sqrt(Math.max(0, slots.length - 1) + 0.5) * spreads[p] * 0.35 + Math.sqrt(biggest) * 4 * SEED_SPREAD;
  });
  const ring = parts > 1 ? (Math.max(...reach) * 1.15) / Math.sin(Math.PI / parts) : 0;
  members.forEach((lists, p) => {
    const ox = parts > 1 ? Math.cos((p / parts) * Math.PI * 2) * ring : 0;
    const oy = parts > 1 ? Math.sin((p / parts) * Math.PI * 2) * ring : 0;
    let slot = 0;
    for (const list of lists) {
      if (list.length === 0) continue;
      const r = Math.sqrt(slot + 0.5) * spreads[p] * 0.35;
      const cx = ox + Math.cos(slot * golden) * r;
      const cy = oy + Math.sin(slot * golden) * r;
      const disc = Math.sqrt(list.length) * 4 * SEED_SPREAD;
      for (const i of list) {
        const angle = random() * Math.PI * 2;
        const dist = Math.sqrt(random()) * disc;
        positions[i * 2] = cx + Math.cos(angle) * dist;
        positions[i * 2 + 1] = cy + Math.sin(angle) * dist;
      }
      slot++;
    }
  });
  return positions;
}

/** The nodes already on screen when an agent opens more graphs (ws009 M1b1): their positions, by index, and 1 for
 * each one kept there. */
export type Keep = { positions: Float32Array; fixed: Uint8Array };

/**
 * Seeds for a layout that keeps the nodes already on screen where they are (ws009 M1b1, ui-expert design point 1):
 * those take their kept positions; the new graphs' seeds (still on their own ring regions, so apart from each other)
 * move together to the right of the kept nodes, so no new node starts on top of an old one.
 */
export function keepSeeds(seeded: Float32Array, keep: Keep): Float32Array {
  const out = seeded.slice();
  const n = keep.fixed.length;
  let [kx0, kx1, ky0, ky1] = [Infinity, -Infinity, Infinity, -Infinity];
  let [nx0, nx1, ny0, ny1] = [Infinity, -Infinity, Infinity, -Infinity];
  for (let i = 0; i < n; i++) {
    const [x, y] = keep.fixed[i] ? [keep.positions[i * 2], keep.positions[i * 2 + 1]] : [seeded[i * 2], seeded[i * 2 + 1]];
    if (keep.fixed[i]) [kx0, kx1, ky0, ky1] = [Math.min(kx0, x), Math.max(kx1, x), Math.min(ky0, y), Math.max(ky1, y)];
    else [nx0, nx1, ny0, ny1] = [Math.min(nx0, x), Math.max(nx1, x), Math.min(ny0, y), Math.max(ny1, y)];
  }
  if (kx0 === Infinity || nx0 === Infinity) {
    for (let i = 0; i < n; i++) if (keep.fixed[i]) [out[i * 2], out[i * 2 + 1]] = [keep.positions[i * 2], keep.positions[i * 2 + 1]];
    return out;
  }
  const gap = Math.max(kx1 - kx0, ky1 - ky0, 1) * 0.15;
  const dx = kx1 + gap - nx0;
  const dy = (ky0 + ky1) / 2 - (ny0 + ny1) / 2;
  for (let i = 0; i < n; i++) {
    if (keep.fixed[i]) [out[i * 2], out[i * 2 + 1]] = [keep.positions[i * 2], keep.positions[i * 2 + 1]];
    else [out[i * 2], out[i * 2 + 1]] = [seeded[i * 2] + dx, seeded[i * 2 + 1] + dy];
  }
  return out;
}

/**
 * A graph's layout from its groups and its LayoutSeed: seeded (beside the kept nodes, when an agent opened more
 * graphs), then run. `onTick` first gets the seed itself.
 */
export function layOut(
  g: IndexGraph,
  groups: Groups,
  sizes: Float32Array,
  seed: LayoutSeed,
  budgetMs: number,
  onTick?: (positions: Float32Array, progress: number) => void,
  keep?: Keep,
): LayoutResult {
  const seeded = keep ? keepSeeds(seedPositions(g, groups, seed.seed), keep) : seedPositions(g, groups, seed.seed);
  onTick?.(seeded.slice(), 0);
  return runLayout(g, sizes, seeded, budgetMs, seed.spacing, onTick, keep?.fixed, groups);
}

/** How many points along its ray compactGroups tries for each group. */
const PULL_STEPS = 48;

/**
 * Pulls the groups toward the layout's centre along their centroid rays, nearest first, each to the first point on
 * its ray where its circle (its centroid and farthest member, with that member's size) stays `gap` clear of every other
 * group where it is now, moved or not (the ui-expert's spacing lever, plan 198). ForceAtlas2 leaves loosely joined
 * groups far apart; this closes the empty space between them, so at Fit the nodes are drawn further apart. A group
 * never moves outward and never into another; one with no clear point stays. Nodes in no group stay put. Moves
 * `positions` in place.
 */
export function compactGroups(positions: Float32Array, sizes: Float32Array, group: Int32Array, gap: number) {
  const count = group.reduce((m, v) => Math.max(m, v + 1), 0);
  const [sx, sy, k, radius] = [new Float64Array(count), new Float64Array(count), new Float64Array(count), new Float64Array(count)];
  let [cx, cy, total] = [0, 0, 0];
  for (let i = 0; i < group.length; i++) {
    const r = group[i];
    if (r < 0) continue;
    sx[r] += positions[i * 2];
    sy[r] += positions[i * 2 + 1];
    k[r]++;
    cx += positions[i * 2];
    cy += positions[i * 2 + 1];
    total++;
  }
  if (total === 0) return;
  [cx, cy] = [cx / total, cy / total];
  for (let r = 0; r < count; r++) if (k[r] > 0) [sx[r], sy[r]] = [sx[r] / k[r], sy[r] / k[r]];
  for (let i = 0; i < group.length; i++) {
    const r = group[i];
    if (r >= 0) radius[r] = Math.max(radius[r], Math.hypot(positions[i * 2] - sx[r], positions[i * 2 + 1] - sy[r]) + sizes[i]);
  }
  const live = [...Array(count).keys()].filter((r) => k[r] > 0);
  const order = [...live].sort((a, b) => Math.hypot(sx[a] - cx, sy[a] - cy) - Math.hypot(sx[b] - cx, sy[b] - cy) || a - b);
  // Where each group's centre is now: its centroid until it moves.
  const [ax, ay] = [Float64Array.from(sx), Float64Array.from(sy)];
  const clear = (r: number, x: number, y: number) => {
    for (const o of live) if (o !== r && Math.hypot(x - ax[o], y - ay[o]) < radius[r] + radius[o] + gap) return false;
    return true;
  };
  for (const r of order) {
    const d = Math.hypot(sx[r] - cx, sy[r] - cy);
    if (d === 0) continue;
    for (let s = 0; s < PULL_STEPS; s++) {
      const t = (d * s) / PULL_STEPS;
      const [tx, ty] = [cx + ((sx[r] - cx) / d) * t, cy + ((sy[r] - cy) / d) * t];
      if (clear(r, tx, ty)) {
        [ax[r], ay[r]] = [tx, ty];
        break;
      }
    }
  }
  for (let i = 0; i < group.length; i++) {
    const r = group[i];
    if (r < 0) continue;
    positions[i * 2] += ax[r] - sx[r];
    positions[i * 2 + 1] += ay[r] - sy[r];
  }
}

/** Spacing increases repulsion and therefore the equilibrium link distance; strong gravity keeps groups together.
 * Up to `over` that is all it does. Sigma draws nodes at a fixed screen size, so a layout that only grows looks the
 * same at Fit; above `over`, gravity grows against attraction by (spacing / over) ** `even`, which spreads the
 * nodes more evenly (dense groups open up, empty space between groups closes), and repulsion grows to match, so the
 * layout's size keeps growing in step with the spacing (10 times at 30 against 3). */
export const FORCE = { scaling: 3, gravity: 2, over: SPACING_LINEAR, even: 2.5 } as const;

/** ForceAtlas2 for the spread-out look: more repulsion, strong gravity, attraction shared out by degree (so hubs do
 * not pull their leaves into a ball), no linLog (it tightens clusters) and no size adjustment during the run
 * (Barnes-Hut runs above 2k nodes); noverlap separates nodes after it. */
function forceSettings(graph: Graph, spacing: number) {
  const inferred = forceAtlas2.inferSettings(graph);
  const over = Math.max(1, spacing / FORCE.over);
  return {
    ...inferred,
    scalingRatio: (inferred.scalingRatio ?? 1) * FORCE.scaling * spacing * over ** (FORCE.even + 1),
    gravity: (inferred.gravity ?? 1) * FORCE.gravity * over ** FORCE.even,
    strongGravityMode: true,
    outboundAttractionDistribution: true,
    barnesHutOptimize: graph.order > 2000,
    barnesHutTheta: 0.6,
    adjustSizes: false,
    linLogMode: false,
  };
}

/** noverlap's margin, in median node sizes at spacing 1, and its ratio. */
export const NOVERLAP = { margin: 1.5, ratio: 1.5 } as const;

/** The median of the node sizes (noverlap's margin and the gap compactGroups leaves are shares of it). */
function medianSize(sizes: Float32Array): number {
  if (sizes.length === 0) return 1;
  const sorted = Float32Array.from(sizes).sort();
  return sorted[sorted.length >> 1];
}

/** The gap compactGroups leaves between groups, in median node sizes. */
export const PULL_GAP = 1.5;

/** Runs ForceAtlas2 from `seeded` (a seed, or the positions on screen when the spacing changes), pulls the groups
 * together when they are given and no node is kept in place, then noverlap. */
export function runLayout(
  g: IndexGraph,
  sizes: Float32Array,
  seeded: Float32Array,
  budgetMs: number,
  spacing: number,
  onTick?: (positions: Float32Array, progress: number) => void,
  fixed?: Uint8Array,
  groups?: Groups,
): LayoutResult {
  const n = g.n;
  const graph = new Graph({ type: "undirected", multi: true, allowSelfLoops: false });
  for (let i = 0; i < n; i++) graph.addNode(i, { x: seeded[i * 2], y: seeded[i * 2 + 1], size: sizes[i], fixed: fixed?.[i] === 1 });
  for (let e = 0; e < g.edges.length; e += 2) {
    if (g.edges[e] !== g.edges[e + 1]) graph.addEdge(g.edges[e], g.edges[e + 1]);
  }
  anchorParts(graph, g, seeded);
  const settings = forceSettings(graph, spacing);
  const maxIterations = iterationsFor(n);
  const chunk = n > 5000 ? 4 : n > 1000 ? 15 : 60;
  const started = performance.now();
  let iterations = 0;
  let capped = false;
  while (iterations < maxIterations) {
    if (performance.now() - started >= budgetMs) {
      capped = true;
      break;
    }
    const step = Math.min(chunk, maxIterations - iterations);
    forceAtlas2.assign(graph, { iterations: step, settings });
    iterations += step;
    onTick?.(positionsOf(graph, n), Math.min(0.95, iterations / maxIterations));
  }
  const median = medianSize(sizes);
  if (groups && groups.of.length === n && groups.sizes.length <= 128 && !fixed?.includes(1)) {
    const pulled = positionsOf(graph, n);
    compactGroups(pulled, sizes, homeGroups(g, groups), PULL_GAP * median);
    for (let i = 0; i < n; i++) graph.mergeNodeAttributes(i, { x: pulled[i * 2], y: pulled[i * 2 + 1] });
  }
  noverlap.assign(graph, { maxIterations: n > 5000 ? 15 : 60, settings: { margin: NOVERLAP.margin * median * spacing, ratio: NOVERLAP.ratio, speed: 3 } });
  const positions = positionsOf(graph, n);
  // noverlap moves every node a little; a kept node stays exactly where it was (ws009 M1b1).
  if (fixed) for (let i = 0; i < n; i++) if (fixed[i]) [positions[i * 2], positions[i * 2 + 1]] = [seeded[i * 2], seeded[i * 2 + 1]];
  const clear = separateDiscs(Array.from({ length: n }, (_, i) => ({
    x: positions[i * 2], y: positions[i * 2 + 1], radius: clearanceRadius(sizes[i], spacing),
  })), fixed);
  clear.forEach((p, i) => { [positions[i * 2], positions[i * 2 + 1]] = [p.x, p.y]; });
  return { positions, iterations, ms: Math.round(performance.now() - started), capped };
}

/** How hard a graph's anchor pulls its nodes together, against an edge's 1, for a graph of ANCHOR_BASE nodes. */
export const ANCHOR_WEIGHT = 0.15;
export const ANCHOR_BASE = 300;

/** A bigger graph pushes itself apart harder (ForceAtlas2 repulsion grows with its node count), so its anchor pulls
 * harder too: with the square root of its size over ANCHOR_BASE, never less than ANCHOR_WEIGHT (review N1). */
export function anchorWeight(size: number): number {
  return ANCHOR_WEIGHT * Math.max(1, Math.sqrt(size / ANCHOR_BASE));
}

/**
 * Several graphs open (plan 133): one anchor node per graph, at its seed's centre, joined to each of its nodes by a
 * weak edge, so ForceAtlas2 keeps each graph together and its hull compact even when only external edges join its
 * parts. Anchors are numbered after the real nodes and never leave the layout (positionsOf reads the first n). One
 * graph gets none, so it lays out exactly as before.
 */
function anchorParts(graph: Graph, g: IndexGraph, seeded: Float32Array) {
  if (!g.part) return;
  const parts = g.part.reduce((m, v) => Math.max(m, v), 0) + 1;
  if (parts < 2) return;
  const sum = Array.from({ length: parts }, () => [0, 0, 0]);
  g.part.forEach((p, i) => {
    sum[p][0] += seeded[i * 2];
    sum[p][1] += seeded[i * 2 + 1];
    sum[p][2]++;
  });
  graph.forEachEdge((e) => graph.setEdgeAttribute(e, "weight", 1));
  sum.forEach(([x, y, k], p) => graph.addNode(g.n + p, { x: k > 0 ? x / k : 0, y: k > 0 ? y / k : 0, size: 1 }));
  g.part.forEach((p, i) => graph.addEdge(i, g.n + p, { weight: anchorWeight(sum[p][2]) }));
}

function positionsOf(graph: Graph, n: number): Float32Array {
  const out = new Float32Array(n * 2);
  for (let i = 0; i < n; i++) {
    const a = graph.getNodeAttributes(i);
    out[i * 2] = a.x;
    out[i * 2 + 1] = a.y;
  }
  return out;
}

/** How many ForceAtlas2 steps a nudge takes: enough to pull a new node to its neighbours, not to move the graph. */
export const NUDGE_ITERATIONS = 40;

/**
 * A short, gentle ForceAtlas2 on a small part of the graph (lib/graph/apply nudgeInput): nodes marked fixed never
 * move, so the graph the person was looking at stays where it was; only the new nodes settle between their
 * neighbours and away from the nodes around them. No gravity: it would pull them towards the middle of the graph.
 */
export function nudge(input: { n: number; edges: Uint32Array; positions: Float32Array; sizes: Float32Array; fixed: Uint8Array; spacing?: number }): Float32Array {
  const { n, edges, positions, sizes, fixed } = input;
  const graph = new Graph({ type: "undirected", multi: true, allowSelfLoops: false });
  for (let i = 0; i < n; i++) graph.addNode(i, { x: positions[i * 2], y: positions[i * 2 + 1], size: sizes[i], fixed: fixed[i] === 1 });
  for (let e = 0; e < edges.length; e += 2) {
    if (edges[e] !== edges[e + 1]) graph.addEdge(edges[e], edges[e + 1]);
  }
  const settings = { ...forceAtlas2.inferSettings(graph), gravity: 0, slowDown: 4, barnesHutOptimize: n > 2000, adjustSizes: false };
  forceAtlas2.assign(graph, { iterations: NUDGE_ITERATIONS, settings });
  const settled = positionsOf(graph, n);
  const clear = separateDiscs(Array.from({ length: n }, (_, i) => ({
    x: settled[i * 2], y: settled[i * 2 + 1], radius: clearanceRadius(sizes[i], input.spacing ?? 1.6),
  })), fixed);
  return Float32Array.from(clear.flatMap((p) => [p.x, p.y]));
}

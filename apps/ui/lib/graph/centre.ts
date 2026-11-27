// Centre on this node (PRD 005): a view-only layout in rings of hops around one node, over every open graph. One
// breadth-first search plus ring placement, no force pass, so it stays fast on big views. Pure, so tests run it.
import type { ViewGraph } from "@/lib/graph/build";
import { clearanceRadius } from "@/lib/graph/clearance";
import type { IndexGraph } from "@/lib/graph/clusters";
import { type Entities, MERGED_GROWTH } from "@/lib/graph/entities";
import { hubSize } from "@/lib/graph/hubs";

/** A node the view does not show: it gets no place and joins no rings. */
export const NOT_PLACED = -1;
/** A shown node no shown link connects to the centre: it sits on the shelf outside the last ring. */
export const SHELF = -2;

export type CentreInput = {
  graph: IndexGraph;
  /** Graph-qualified keys by index: the last tie-break. */
  ids: readonly string[];
  /** Node types by index: ordered before the id within a ring. */
  kinds: readonly string[];
  /** 1 for each node the view shows. */
  shown: Uint8Array;
  /** 1 for each edge whose relation is shown (build.ts edgeMask). */
  edges: Uint8Array;
  /** Each node's clearance radius in layout units: two placed nodes are never closer than the sum of theirs. */
  radii: Float32Array;
  centre: number;
  /** Merged entities (PRD 005 centre-16): the index of the node that stands for each node's entity, the node itself
   * when it is in none. A unit takes one place, at the hop of its nearest member, so no entity spans two rings. */
  unit?: Int32Array;
  /** The members of the centre's unit whose links start the rings: all its shown members by default, one member when
   * the person chose that member on its own. */
  seeds?: readonly number[];
};

/** `hop[i]`: the ring of node i (0 is the centre's unit), else SHELF or NOT_PLACED. Members of one unit share a
 * position; positions of unplaced nodes are 0. */
export type CentrePlan = { positions: Float32Array; hop: Int32Array; rings: number; shelf: number };

const compare = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

/** Air between one ring and the next, in that pair's largest clearance radius; the shelf gets more. */
const RING_GAP = 1;
const SHELF_GAP = 4;
/** Chords are kept this much over a node pair's reach, so rounding never makes two touch. */
const SLACK = 1.02;

/**
 * Lays `list` in a band of concentric circles from `inner` outward (the hub fans' rule, PRD 004 hub-08: a crowded
 * band grows rather than stacking): each circle holds its share evenly spaced, the circles one diameter apart, with the
 * number of circles that keeps the band thinnest. Item j goes on circle j mod S, so the band keeps the list's order
 * around the circle. Returns the band's outer edge.
 */
function band(list: readonly number[], inner: number, radius: (u: number) => number, put: (u: number, x: number, y: number) => void, start: number): number {
  const n = list.length;
  if (n === 0) return inner;
  const reach = list.reduce((m, u) => Math.max(m, radius(u)), 0);
  const pitch = 2 * reach * SLACK;
  const first = inner + reach;
  // A circle holding m evenly spaced items needs a chord of at least pitch between neighbours.
  const need = (m: number) => (m <= 1 ? 0 : pitch / (2 * Math.sin(Math.PI / m)));
  let [circles, r0, best] = [1, Math.max(first, need(n)), Infinity];
  for (let s = 1; s <= n && first + (s - 1) * pitch < best; s++) {
    const r = Math.max(first, need(Math.ceil(n / s)));
    const edge = r + (s - 1) * pitch;
    if (edge < best) [circles, r0, best] = [s, r, edge];
  }
  for (let s = 0; s < circles; s++) {
    const count = Math.ceil((n - s) / circles);
    const r = r0 + s * pitch;
    for (let t = 0; t < count; t++) {
      const angle = start + (Math.PI * 2 * (t + 0.5)) / count;
      put(list[s + t * circles], Math.cos(angle) * r, Math.sin(angle) * r);
    }
  }
  return best + reach;
}

/**
 * The centre's unit at the origin, ring k holding the shown units k shown links away (links either way, across
 * graphs), and the shown units it cannot reach on a shelf beyond the last ring. Within ring 1, type then id; further
 * out, the place of the unit that reached it in the ring inside first, then type, then id. A unit is named by its
 * member with the smallest id. Null when the centre is not shown.
 */
export function centreRings(input: CentreInput): CentrePlan | null {
  const { graph: g, ids, kinds, shown, edges, radii, centre } = input;
  const n = g.n;
  if (centre < 0 || centre >= n || !shown[centre]) return null;
  const unit = input.unit ?? Int32Array.from({ length: n }, (_, i) => i);
  // Adjacency between shown nodes over shown links, as offsets into `next`.
  const degree = new Int32Array(n + 1);
  const ends = g.edges;
  const usable = (e: number) => {
    const a = ends[e * 2], b = ends[e * 2 + 1];
    return edges[e] === 1 && a !== b && shown[a] === 1 && shown[b] === 1;
  };
  const m = ends.length / 2;
  for (let e = 0; e < m; e++) if (usable(e)) { degree[ends[e * 2] + 1]++; degree[ends[e * 2 + 1] + 1]++; }
  for (let i = 0; i < n; i++) degree[i + 1] += degree[i];
  const next = new Int32Array(degree[n]);
  const fill = degree.slice(0, n);
  for (let e = 0; e < m; e++) {
    if (!usable(e)) continue;
    const a = ends[e * 2], b = ends[e * 2 + 1];
    next[fill[a]++] = b;
    next[fill[b]++] = a;
  }
  // Each unit's shown members, and the member that names it.
  const members = new Map<number, number[]>();
  for (let i = 0; i < n; i++) if (shown[i]) {
    const list = members.get(unit[i]);
    if (list) list.push(i);
    else members.set(unit[i], [i]);
  }
  const name = new Map<number, number>();
  for (const [u, list] of members) name.set(u, list.reduce((a, b) => (compare(ids[b], ids[a]) < 0 ? b : a)));
  const reach = new Map<number, number>();
  for (const [u, list] of members) reach.set(u, list.reduce((r, i) => Math.max(r, radii[i]), 0));
  const hop = new Map<number, number>();
  const queue = new Int32Array(n);
  let [head, tail] = [0, 0];
  const home = unit[centre];
  hop.set(home, 0);
  const seeds = (input.seeds ?? members.get(home)!).filter((i) => shown[i] && unit[i] === home);
  for (const i of seeds.length > 0 ? seeds : [centre]) queue[tail++] = i;
  const rings: number[][] = [[home]];
  while (head < tail) {
    const i = queue[head++];
    const h = hop.get(unit[i])!;
    for (let k = degree[i]; k < degree[i + 1]; k++) {
      const u = unit[next[k]];
      if (hop.has(u)) continue;
      hop.set(u, h + 1);
      (rings[h + 1] ??= []).push(u);
      for (const j of members.get(u)!) queue[tail++] = j;
    }
  }
  const byName = (a: number, b: number) => {
    const [x, y] = [name.get(a)!, name.get(b)!];
    return compare(kinds[x], kinds[y]) || compare(ids[x], ids[y]);
  };
  const place = new Map<number, number>([[home, 0]]);
  for (let k = 1; k < rings.length; k++) {
    const parent = new Map<number, number>();
    for (const u of rings[k]) {
      let p = Infinity;
      for (const i of members.get(u)!) {
        for (let e = degree[i]; e < degree[i + 1]; e++) {
          const v = unit[next[e]];
          if (hop.get(v) === k - 1) p = Math.min(p, place.get(v)!);
        }
      }
      parent.set(u, p);
    }
    rings[k].sort((a, b) => parent.get(a)! - parent.get(b)! || byName(a, b));
    rings[k].forEach((u, at) => place.set(u, at));
  }
  const shelf = [...members.keys()].filter((u) => !hop.has(u)).sort(byName);
  const positions = new Float32Array(n * 2);
  const out = new Int32Array(n).fill(NOT_PLACED);
  const put = (u: number, x: number, y: number) => {
    for (const i of members.get(u)!) [positions[i * 2], positions[i * 2 + 1]] = [x, y];
  };
  const radius = (u: number) => reach.get(u)!;
  const top = -Math.PI / 2;
  let outer = radius(home);
  let last = outer;
  for (let k = 1; k < rings.length; k++) {
    const r = rings[k].reduce((a, u) => Math.max(a, radius(u)), 0);
    outer = band(rings[k], outer + RING_GAP * Math.max(last, r), radius, put, top);
    last = r;
  }
  if (shelf.length > 0) {
    const r = shelf.reduce((a, u) => Math.max(a, radius(u)), 0);
    band(shelf, outer + SHELF_GAP * Math.max(last, r), radius, put, top);
  }
  for (const [u, list] of members) for (const i of list) out[i] = hop.get(u) ?? SHELF;
  return { positions, hop: out, rings: rings.length - 1, shelf: shelf.length };
}

/** What the view is centred on (PRD 005): a node, by its canvas key; `member` when the person chose that member of a
 * merged entity on its own, so only its links start the rings. */
export type CentreOn = { key: string; member: boolean };

/**
 * centreRings over the canvas: the nodes `shown` (build.ts shownMask) and the links `edges` keeps, each node's drawn
 * clearance, and, while entities are merged, each entity of two or more shown members as one unit, as wide as its
 * merged glyph (entities.ts mergedPlaces).
 */
export function centreView(
  graph: ViewGraph,
  index: IndexGraph,
  ids: readonly string[],
  shown: Uint8Array,
  edges: Uint8Array,
  on: CentreOn,
  entities: Entities,
  spacing: number,
): CentrePlan | null {
  if (!graph.hasNode(on.key)) return null;
  const n = index.n;
  const kinds = new Array<string>(n).fill("");
  const radii = new Float32Array(n);
  graph.forEachNode((_k, a) => {
    kinds[a.index] = a.kind;
    radii[a.index] = a.clearance ?? clearanceRadius(a.hub ? hubSize(a.size) : a.size, spacing);
  });
  const unit = Int32Array.from({ length: n }, (_, i) => i);
  const at = (key: string) => graph.getNodeAttribute(key, "index");
  for (const e of entities.list) {
    const members = e.members.filter((m) => graph.hasNode(m) && shown[at(m)]).map(at);
    if (members.length < 2) continue;
    const head = Math.min(...members);
    for (const i of members) {
      unit[i] = head;
      radii[i] *= MERGED_GROWTH;
    }
  }
  const centre = at(on.key);
  return centreRings({ graph: index, ids, kinds, shown, edges, radii, centre, unit, ...(on.member ? { seeds: [centre] } : {}) });
}

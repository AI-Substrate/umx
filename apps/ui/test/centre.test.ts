import { describe, expect, it } from "vitest";
import { centreRings, type CentreInput, NOT_PLACED, SHELF } from "@/lib/graph/centre";
import type { IndexGraph } from "@/lib/graph/clusters";
import { rng } from "@/lib/graph/rng";

function graph(n: number, rows: [number, number][]): IndexGraph {
  return { n, edges: Uint32Array.from(rows.flat()), partOf: new Uint8Array(rows.length) };
}
const ones = (n: number) => new Uint8Array(n).fill(1);
function input(n: number, rows: [number, number][], centre: number, extra: Partial<CentreInput> = {}): CentreInput {
  return {
    graph: graph(n, rows), ids: Array.from({ length: n }, (_, i) => `g/${String(i).padStart(5, "0")}`),
    kinds: Array.from({ length: n }, () => "thing"), shown: ones(n), edges: ones(rows.length),
    radii: new Float32Array(n).fill(5), centre, ...extra,
  };
}
const distance = (p: Float32Array, i: number) => Math.hypot(p[i * 2], p[i * 2 + 1]);
function clear(p: Float32Array, radii: Float32Array, placed: (i: number) => boolean) {
  const n = radii.length;
  for (let i = 0; i < n; i++) for (let j = 0; j < i; j++) {
    if (!placed(i) || !placed(j)) continue;
    const d = Math.hypot(p[i * 2] - p[j * 2], p[i * 2 + 1] - p[j * 2 + 1]);
    if (d < radii[i] + radii[j]) throw new Error(`overlap ${i} ${j}: ${d}`);
  }
}

describe("centre on this node (PRD 005)", () => {
  it("puts every node on the ring of its undirected hop count, across a cross-graph link (centre-02)", () => {
    // Graph a: 0-1-2, 0<-3; graph b: 4-5, joined to a only by 2->4 (a cross-graph link is an ordinary edge here).
    const plan = centreRings(input(6, [[0, 1], [1, 2], [3, 0], [2, 4], [4, 5]], 0))!;
    expect([...plan.hop]).toEqual([0, 1, 2, 1, 3, 4]);
    expect(plan.rings).toBe(4);
    expect([plan.positions[0], plan.positions[1]]).toEqual([0, 0]);
  });

  it("keeps rings apart and never overlaps, with crowded rings and mixed sizes (centre-03)", () => {
    const rows: [number, number][] = [];
    for (let i = 1; i <= 500; i++) rows.push([0, i]);
    rows.push([1, 501], [501, 502]);
    const radii = Float32Array.from({ length: 503 }, (_, i) => 3 + (i % 7) * 2);
    const plan = centreRings(input(503, rows, 0, { radii }))!;
    clear(plan.positions, radii, () => true);
    const ring = (k: number) => [...plan.hop.keys()].filter((i) => plan.hop[i] === k).map((i) => distance(plan.positions, i));
    for (let k = 1; k <= 3; k++) expect(Math.min(...ring(k))).toBeGreaterThan(Math.max(...ring(k - 1)));
    // A crowded ring is a band of circles, not one huge circle: 500 nodes of reach 15 fit well inside 30 node widths.
    expect(Math.max(...ring(1))).toBeLessThan(30 * 2 * 15);
  });

  it("is the same for the same node, whatever order the graph arrived in (centre-04)", () => {
    const n = 60;
    const random = rng(7);
    const rows: [number, number][] = [];
    for (let i = 1; i < n; i++) rows.push([Math.floor(random() * i), i]);
    for (let k = 0; k < 30; k++) rows.push([Math.floor(random() * n), Math.floor(random() * n)]);
    const kinds = Array.from({ length: n }, (_, i) => (i % 3 === 0 ? "risk" : "issue"));
    const one = centreRings(input(n, rows, 5, { kinds }))!;
    expect(centreRings(input(n, rows, 5, { kinds }))!.positions).toEqual(one.positions);
    // The same graph with its nodes renumbered and its edges shuffled.
    const order = Array.from({ length: n }, (_, i) => i).sort(() => random() - 0.5);
    const at = new Int32Array(n);
    order.forEach((old, i) => { at[old] = i; });
    const base = input(n, rows, 5, { kinds });
    const shuffled = centreRings({
      ...base,
      graph: graph(n, rows.map(([a, b]): [number, number] => [at[a], at[b]]).sort(() => random() - 0.5)),
      ids: order.map((old) => base.ids[old]),
      kinds: order.map((old) => kinds[old]),
      centre: at[5],
    })!;
    for (let i = 0; i < n; i++) {
      expect([shuffled.positions[at[i] * 2], shuffled.positions[at[i] * 2 + 1]]).toEqual([one.positions[i * 2], one.positions[i * 2 + 1]]);
    }
  });

  it("orders ring 1 by type then id, and outer rings by the place of their parent first", () => {
    // Centre 0; ring 1: 3 (alpha), 1 (beta), 2 (beta); ring 2: 4 under 2, 5 under 3.
    const kinds = ["c", "beta", "beta", "alpha", "x", "x"];
    const plan = centreRings(input(6, [[0, 1], [0, 2], [0, 3], [2, 4], [3, 5]], 0, { kinds }))!;
    const angle = (i: number) => (Math.atan2(plan.positions[i * 2 + 1], plan.positions[i * 2]) + Math.PI * 2.5) % (Math.PI * 2);
    expect([3, 1, 2].map(angle)).toEqual([...[3, 1, 2].map(angle)].sort((a, b) => a - b));
    // 5's parent (3) is first on ring 1, so 5 comes before 4 on ring 2.
    expect(angle(5)).toBeLessThan(angle(4));
  });

  it("puts unconnected nodes on a shelf outside the last ring (centre-05)", () => {
    const plan = centreRings(input(6, [[0, 1], [1, 2], [3, 4]], 0))!;
    expect([...plan.hop]).toEqual([0, 1, 2, SHELF, SHELF, SHELF]);
    expect(plan.shelf).toBe(3);
    const outerRing = distance(plan.positions, 2);
    for (const i of [3, 4, 5]) expect(distance(plan.positions, i)).toBeGreaterThan(outerRing + 10);
    clear(plan.positions, new Float32Array(6).fill(5), () => true);
  });

  it("uses only shown nodes and shown relations (centre-06)", () => {
    // 0-1-2 by relation edge 0 and 1; 0-3-4 with 3 hidden.
    const rows: [number, number][] = [[0, 1], [1, 2], [0, 3], [3, 4]];
    const shown = Uint8Array.from([1, 1, 1, 0, 1]);
    const plan = centreRings(input(5, rows, 0, { shown, edges: Uint8Array.from([1, 0, 1, 1]) }))!;
    expect([...plan.hop]).toEqual([0, 1, SHELF, NOT_PLACED, SHELF]);
    expect(centreRings(input(5, rows, 3, { shown }))).toBeNull();
  });

  it("treats a merged entity as one place, from all members' links or one member's (centre-16)", () => {
    // Entity {0, 1}: 0-2, 1-3. Entity {4, 5}: 4 two hops out (via 2), 5 three hops out (via 3-6).
    const rows: [number, number][] = [[0, 2], [1, 3], [2, 4], [3, 6], [6, 5]];
    const unit = Int32Array.from([0, 0, 2, 3, 4, 4, 6]);
    const all = centreRings(input(7, rows, 0, { unit }))!;
    expect([...all.hop]).toEqual([0, 0, 1, 1, 2, 2, 2]);
    expect([all.positions[8], all.positions[9]]).toEqual([all.positions[10], all.positions[11]]);
    const one = centreRings(input(7, rows, 1, { unit, seeds: [1] }))!;
    expect([...one.hop]).toEqual([0, 0, 4, 1, 3, 3, 2]);
  });

  it("lays out 10,000 nodes and 30,000 links quickly, with no force pass (centre-12)", () => {
    const n = 10_000;
    const random = rng(11);
    const rows: [number, number][] = [];
    for (let i = 1; i < n; i++) rows.push([Math.floor(random() * i), i]);
    while (rows.length < 30_000) rows.push([Math.floor(random() * n), Math.floor(random() * n)]);
    const radii = Float32Array.from({ length: n }, () => 3 + random() * 6);
    const started = performance.now();
    const plan = centreRings(input(n, rows, 0, { radii }))!;
    const ms = performance.now() - started;
    console.info(`centreRings: ${n} nodes, ${rows.length} links, ${plan.rings} rings in ${ms.toFixed(1)} ms`);
    expect(plan.shelf).toBe(0);
    expect(ms).toBeLessThan(1_000);
  });
});

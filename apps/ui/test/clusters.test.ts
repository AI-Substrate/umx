import { describe, expect, it } from "vitest";
import { convexHull, type IndexGraph, louvainGroups, parentGroups } from "@/lib/graph/clusters";
import { compactGroups, iterationsFor, keepSeeds, layOut, runLayout, seedPositions } from "@/lib/graph/layout";
import { SPACING, spacingOf } from "@/lib/graph/spacing";

/** Two 6-cliques joined by one edge, plus node 12 hanging off node 0 by part_of. */
function twoCliques(): IndexGraph {
  const edges: number[] = [];
  const partOf: number[] = [];
  for (const base of [0, 6]) {
    for (let i = 0; i < 6; i++) {
      for (let j = i + 1; j < 6; j++) {
        edges.push(base + i, base + j);
        partOf.push(0);
      }
    }
  }
  edges.push(0, 6);
  partOf.push(0);
  edges.push(12, 0);
  partOf.push(1);
  return { n: 13, edges: Uint32Array.from(edges), partOf: Uint8Array.from(partOf) };
}

const all = (n: number) => new Uint8Array(n).fill(1);

describe("louvain groups", () => {
  it("finds the two cliques, the same way every run", () => {
    const g = twoCliques();
    const a = louvainGroups(g, all(13), 1, 7);
    const b = louvainGroups(g, all(13), 1, 7);
    expect([...a.of]).toEqual([...b.of]);
    expect(new Set(a.of.slice(0, 6)).size).toBe(1);
    expect(new Set(a.of.slice(6, 12)).size).toBe(1);
    expect(a.of[0]).not.toBe(a.of[6]);
    expect(a.sizes[0]).toBeGreaterThanOrEqual(a.sizes[1]);
  });

  it("leaves hidden nodes out", () => {
    const visible = all(13);
    visible[12] = 0;
    expect(louvainGroups(twoCliques(), visible, 1, 7).of[12]).toBe(-1);
  });

  it("weights part_of up", () => {
    // 12 is tied to 0 by part_of and to 6 by a plain edge: part_of wins.
    const g = twoCliques();
    const edges = Uint32Array.from([...g.edges, 12, 6]);
    const partOf = Uint8Array.from([...g.partOf, 0]);
    const groups = louvainGroups({ n: 13, edges, partOf }, all(13), 1, 7);
    expect(groups.of[12]).toBe(groups.of[0]);
  });
});

describe("parent groups", () => {
  it("groups by the root of the part_of tree, a node with none on its own", () => {
    // 1 -> 0, 2 -> 1, 3 -> 0 by part_of; 4 is linked to 0 by a plain edge only.
    const g: IndexGraph = { n: 5, edges: Uint32Array.from([1, 0, 2, 1, 3, 0, 4, 0]), partOf: Uint8Array.from([1, 1, 1, 0]) };
    const groups = parentGroups(g, all(5));
    expect([...groups.of]).toEqual([0, 0, 0, 0, 1]);
    expect(groups.heads[0]).toBe(0);
    expect(groups.sizes).toEqual([4, 1]);
  });

  it("stops at a hidden parent and survives a cycle", () => {
    const g: IndexGraph = { n: 4, edges: Uint32Array.from([1, 0, 2, 3, 3, 2]), partOf: Uint8Array.from([1, 1, 1]) };
    const visible = Uint8Array.from([0, 1, 1, 1]);
    const groups = parentGroups(g, visible);
    expect(groups.of[0]).toBe(-1);
    expect(groups.of[2]).toBe(groups.of[3]);
    expect(groups.of[1]).not.toBe(groups.of[2]);
  });
});

describe("convex hull", () => {
  it("keeps the corners and drops the inside", () => {
    const hull = convexHull([0, 0, 2, 0, 2, 2, 0, 2, 1, 1, 1, 0]);
    const corners = new Set<string>();
    for (let i = 0; i < hull.length; i += 2) corners.add(`${hull[i]},${hull[i + 1]}`);
    expect(corners).toEqual(new Set(["0,0", "2,0", "2,2", "0,2"]));
  });
});

describe("layout (ui-expert N3)", () => {
  it("lays the same graph out the same way twice: it stops on iterations, not the clock", () => {
    const g = twoCliques();
    const groups = louvainGroups(g, all(13), 1, 3);
    const sizes = new Float32Array(13).fill(4);
    const one = runLayout(g, sizes, seedPositions(g, groups, 3), 60_000, SPACING.default);
    const two = runLayout(g, sizes, seedPositions(g, groups, 3), 60_000, SPACING.default);
    expect(one.capped).toBe(false);
    expect(one.iterations).toBe(iterationsFor(13));
    expect([...one.positions]).toEqual([...two.positions]);
  });

  it("says when the time cap cut it short", () => {
    const g = twoCliques();
    const seeded = seedPositions(g, louvainGroups(g, all(13), 1, 3), 3);
    const capped = runLayout(g, new Float32Array(13).fill(4), seeded, 0, SPACING.default);
    expect(capped.capped).toBe(true);
    expect(capped.iterations).toBeLessThan(iterationsFor(13));
  });
});

describe("spacing (plan 198)", () => {
  const g = twoCliques();
  const groups = louvainGroups(g, all(13), 1, 3);
  const sizes = new Float32Array(13).fill(4);
  /** Mean distance from the layout's centre. */
  const spread = (p: Float32Array) => {
    const [cx, cy] = [0, 1].map((o) => p.filter((_, i) => i % 2 === o).reduce((s, v) => s + v, 0) / 13);
    return [...Array(13).keys()].reduce((s, i) => s + Math.hypot(p[i * 2] - cx, p[i * 2 + 1] - cy), 0) / 13;
  };

  it("lays a graph out the same way at the same seed and spacing, and apart at a wider one", () => {
    const at = (spacing: number) => layOut(g, groups, sizes, { seed: 3, spacing }, 60_000).positions;
    expect([...at(1.6)]).toEqual([...at(1.6)]);
    expect([...at(2.5)]).not.toEqual([...at(1.6)]);
    expect(spread(at(2.5))).toBeGreaterThan(spread(at(1.6)) * 1.1);
    expect(spread(at(1.6))).toBeGreaterThan(spread(at(0.5)) * 1.1);
  });

  it("reads a stored spacing only when it is a number in range", () => {
    expect(spacingOf("2.2")).toBe(2.2);
    expect(spacingOf("0.5")).toBe(0.5);
    expect(spacingOf("3")).toBe(3);
    for (const bad of [null, "", " ", "0.4", "3.1", "wide", "NaN", "Infinity"]) expect(spacingOf(bad)).toBe(SPACING.default);
  });

  it("pulls groups toward the centre until their circles are the gap apart, never outward, and leaves loners", () => {
    // Group 0: two nodes about (0, 0); group 1: two nodes about (100, 0); node 4 in no group, far out.
    const positions = Float32Array.from([-1, 0, 1, 0, 99, 0, 101, 0, 500, 500]);
    const sizes = new Float32Array(5).fill(1);
    compactGroups(positions, sizes, Int32Array.from([0, 0, 1, 1, -1]), 4);
    const [c0, c1] = [(positions[0] + positions[2]) / 2, (positions[4] + positions[6]) / 2];
    // Each circle reaches 2 (1 from the centroid plus the node's size): 4 between the circles, within a step of the ray.
    expect(c1 - c0).toBeGreaterThanOrEqual(2 + 2 + 4);
    expect(c1 - c0).toBeLessThan(2 + 2 + 4 + 100 / 48 + 1e-6);
    expect([positions[2] - positions[0], positions[6] - positions[4]]).toEqual([2, 2]);
    expect([positions[8], positions[9]]).toEqual([500, 500]);
    const apart = Float32Array.from([-1, 0, 1, 0, 6, 0, 8, 0]);
    compactGroups(apart, new Float32Array(4).fill(1), Int32Array.from([0, 0, 1, 1]), 4);
    expect([...apart]).toEqual([-1, 0, 1, 0, 6, 0, 8, 0]);
  });
});

describe("a layout that keeps what is on screen (ws009 M1b1)", () => {
  it("keeps the kept nodes exactly and starts the new ones to the right of them", () => {
    const g = twoCliques();
    const seeded = seedPositions(g, louvainGroups(g, all(13), 1, 3), 3);
    const fixed = new Uint8Array(13).map((_, i) => (i < 6 ? 1 : 0));
    const positions = new Float32Array(26).map((_, i) => (i % 2 === 0 ? i : -i));
    const seeds = keepSeeds(seeded, { positions, fixed });
    const xs = (from: Float32Array, pick: (i: number) => boolean) => [...Array(13).keys()].filter(pick).map((i) => from[i * 2]);
    expect(xs(seeds, (i) => i < 6)).toEqual(xs(positions, (i) => i < 6));
    expect(Math.min(...xs(seeds, (i) => i >= 6))).toBeGreaterThan(Math.max(...xs(positions, (i) => i < 6)));
    const laid = runLayout(g, new Float32Array(13).fill(4), seeds, 60_000, SPACING.default, undefined, fixed);
    expect([...laid.positions.slice(0, 12)]).toEqual([...positions.slice(0, 12)]);
    expect([...laid.positions.slice(12)]).not.toEqual([...seeds.slice(12)]);
  });
});

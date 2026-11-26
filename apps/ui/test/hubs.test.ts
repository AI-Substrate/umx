import { describe, expect, it } from "vitest";
import { canonicalGraph, fanLayout, findHubs, hubSize, type HubOptions } from "@/lib/graph/hubs";
import { clearanceRadius, separateDiscs } from "@/lib/graph/clearance";
import { layOut } from "@/lib/graph/layout";
import { louvainGroups, type IndexGraph } from "@/lib/graph/clusters";
import { spineLayout, type Place } from "@/lib/graph/spine-layout";
import { readLook, writeLook } from "@/lib/view-url";

function graph(n: number, rows: [number, number, number?, number?][]): IndexGraph {
  return {
    n, edges: Uint32Array.from(rows.flatMap(([a, b]) => [a, b])),
    partOf: Uint8Array.from(rows.map((r) => r[2] ?? 0)),
    similarity: Float32Array.from(rows.map((r) => r[3] ?? 0)),
  };
}
const ids = (n: number) => Array.from({ length: n }, (_, i) => `g/${String(i).padStart(5, "0")}`);
const options = (mode: HubOptions["mode"], degree: HubOptions["degree"] = "connections", count = 10): HubOptions => ({ mode, degree, count });
function clear(positions: Float32Array, radii: readonly number[]) {
  for (let i = 0; i < radii.length; i++) for (let j = 0; j < i; j++) {
    if (Math.hypot(positions[i * 2] - positions[j * 2], positions[i * 2 + 1] - positions[j * 2 + 1]) < (radii[i] + radii[j]) * 0.99999) {
      throw new Error(`overlap: ${i}, ${j}`);
    }
  }
}

describe("hub selection", () => {
  it("counts unique directed neighbours, ignores loops, and limits the chosen hubs", () => {
    const g = graph(8, [[1, 0], [2, 0], [3, 0], [4, 5], [4, 6], [4, 7], [0, 0], [1, 0]]);
    expect(findHubs(g, ids(8), options("degree", "incoming", 1), 1, 7).centres).toEqual([0]);
    expect(findHubs(g, ids(8), options("degree", "outgoing", 1), 1, 7).centres).toEqual([4]);
    expect(findHubs(g, ids(8), options("degree", "connections", 1), 1, 7).centres).toEqual([0]);
    expect(findHubs(g, ids(8), options("degree"), 1, 7).centres).toEqual([0, 4]);
  });

  it("uses only incoming part_of for the declared preview, including cyclic parents", () => {
    const g = graph(6, [[1, 0, 1], [2, 0, 1], [0, 3], [0, 4], [3, 5, 1], [4, 5, 1], [5, 0, 1], [0, 5, 1]]);
    const plan = findHubs(g, ids(6), options("declared"), 1, 7);
    expect(plan.centres).toEqual([0, 5]);
    expect([...plan.owner]).toEqual([0, 0, 0, 5, 5, 5]);
  });

  it("centres similarity clusters by total internal score, with served scores overriding unscored edges", () => {
    const g = graph(7, [[0, 1, 0, 1], [1, 2, 0, 1], [0, 2, 0, 1], [3, 4, 0, 1], [4, 5, 0, 1], [3, 5, 0, 1], [6, 0], [6, 1], [6, 3], [6, 4]]);
    const pairs = [
      { a: ids(7)[0], b: ids(7)[1], score: 0.2 }, { a: ids(7)[0], b: ids(7)[2], score: 0.9 }, { a: ids(7)[1], b: ids(7)[2], score: 0.8 },
    ];
    const plan = findHubs(g, ids(7), options("similarity"), 1, 7, pairs);
    expect(new Set(plan.centres)).toEqual(new Set([2, 3]));
    expect(plan.owner[6]).toBe(-1);
    expect(plan.owner[0]).toBe(2);
  });

  it("favours central nodes in degree ties and falls back to plain total degree without similarity", () => {
    const g = graph(6, [[0, 1], [0, 2], [3, 4], [3, 5]]);
    const pairs = [{ a: ids(6)[3], b: ids(6)[4], score: 0.9 }, { a: ids(6)[3], b: ids(6)[5], score: 0.8 }];
    expect(findHubs(g, ids(6), options("degree", "connections", 1), 1, 7, pairs).centres).toEqual([3]);
    expect(findHubs(g, ids(6), options("similarity", "outgoing", 1), 1, 7)).toMatchObject({ centres: [0], fallback: true });
    expect(findHubs(graph(2, []), ids(2), options("similarity"), 1, 7)).toMatchObject({ centres: [], fallback: true });
  });

  it("chooses ten hubs by default, fewer when asked, without inventing candidates", () => {
    const g = graph(36, Array.from({ length: 12 }, (_, i) => [[i * 3, i * 3 + 1], [i * 3, i * 3 + 2]] as [number, number][]).flat());
    expect(findHubs(g, ids(36), options("degree"), 1, 7).centres).toHaveLength(10);
    expect(findHubs(g, ids(36), options("degree", "connections", 3), 1, 7).centres).toHaveLength(3);
  });
});

describe("clearance and fans", () => {
  it("separates 1,000 coincident variable-sized nodes deterministically without moving fixed nodes", () => {
    const discs = Array.from({ length: 1000 }, (_, i) => ({ x: 0, y: 0, radius: 2 + i % 4 }));
    const first = separateDiscs(discs);
    expect(first).toEqual(separateDiscs(discs));
    clear(Float32Array.from(first.flatMap((p) => [p.x, p.y])), discs.map((p) => p.radius));
    expect(separateDiscs(discs, Uint8Array.from(discs.map((_, i) => i === 0 ? 1 : 0)))[0]).toEqual(discs[0]);
  });

  it("keeps dense rings and shared neighbours clear at default and maximum spacing", () => {
    const n = 1000;
    const g = graph(n, Array.from({ length: n - 2 }, (_, i): [number, number] => [i % 2, i + 2]));
    const plan = findHubs(g, ids(n), options("degree"), 1, 7);
    for (const spacing of [1.6, 3]) {
      const sizes = new Float32Array(n).fill(4);
      for (const head of plan.centres) sizes[head] = hubSize(sizes[head]);
      const at = fanLayout(new Float32Array(n * 2), sizes, plan, spacing);
      clear(at, [...sizes].map((s) => clearanceRadius(s, spacing)));
      expect(at.every(Number.isFinite)).toBe(true);
      expect(at.length).toBe(n * 2);
    }
  });

  it("keeps the Normal spine's vertical spread and band geometry when hubs are selected", () => {
    const n = 120;
    const nodes = ids(n).map((key, i) => ({ key, id: String(i), x: i % 7, y: i % 11 }));
    const places = new Map<string, Place>(nodes.map((node, i) => [node.id, { band: i >= 110 ? null : i % 3, by: "field", anchor: i < 3 }]));
    const owners = new Map(nodes.map((node) => [node.key, nodes[0].key]));
    for (const spread of [1, 4]) {
      const result = spineLayout("g", nodes, places, spread, 1.8, owners, 1.6);
      const normal = spineLayout("g", nodes, places, spread, 1.8);
      expect(result.axis).toEqual(normal.axis);
      const height = (at: typeof result.at) => Math.max(...[...at.values()].map((p) => p.y)) - Math.min(...[...at.values()].map((p) => p.y));
      expect(height(result.at)).toBe(height(normal.at));
      const a = result.axis!;
      for (let i = 0; i < 3; i++) expect(result.at.get(nodes[i].key)!.y).toBe(a.y);
      const bandXs = [0, 1, 2].map((band) => nodes.filter((node) => places.get(node.id)!.band === band).map((node) => result.at.get(node.key)!.x));
      expect(Math.max(...bandXs[0])).toBeLessThan(Math.min(...bandXs[1]));
      expect(Math.max(...bandXs[1])).toBeLessThan(Math.min(...bandXs[2]));
      const minPlaced = Math.min(...nodes.slice(0, 110).map((node) => result.at.get(node.key)!.y));
      expect(nodes.slice(110).every((node) => result.at.get(node.key)!.y < minPlaced)).toBe(true);
      clear(Float32Array.from(nodes.flatMap((node) => { const p = result.at.get(node.key)!; return [p.x, p.y]; })), new Array(n).fill(result.clearance));
    }
  });
});

describe("canonical layout and settings", () => {
  it("does not depend on snapshot input order and leaves filtered nodes and edges out", () => {
    const names = ["b/one", "a/one", "a/two", "a/hidden"];
    const g = graph(4, [[0, 1], [1, 2], [2, 0], [3, 0]]);
    const a = canonicalGraph(g, names, Uint8Array.from([1, 1, 1, 0]), Uint8Array.from([1, 1, 0, 1]));
    const b = canonicalGraph(graph(4, [[2, 0], [1, 2], [3, 1], [0, 1]]), ["a/two", "b/one", "a/one", "a/hidden"], Uint8Array.from([1, 1, 1, 0]), Uint8Array.from([1, 1, 1, 0]));
    expect(a.ids).toEqual(b.ids);
    expect(a.graph).toEqual(b.graph);
    const run = (input: typeof a) => {
      const groups = louvainGroups(input.graph, new Uint8Array(input.graph.n).fill(1), 1, 7);
      return layOut(input.graph, groups, new Float32Array(input.graph.n).fill(4), { seed: 7, spacing: 1.6 }, 15_000);
    };
    expect(run(a).positions).toEqual(run(b).positions);
  });

  it("persists mode, direction and count without accepting arbitrary fragment input", () => {
    expect(writeLook({ layout: "normal" })).toBe("");
    expect(readLook(writeLook({ layout: "similarity" }))).toEqual({ layout: "similarity" });
    const look = { layout: "degree" as const, degree: "incoming" as const, hubs: 3 };
    expect(readLook(writeLook(look))).toEqual(look);
    expect(readLook("#layout=script&degree=sideways&hubs=2.5")).toEqual({});
    expect(readLook("#hubs=101")).toEqual({});
  });
});

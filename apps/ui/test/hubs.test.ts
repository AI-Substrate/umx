import { afterEach, describe, expect, it, vi } from "vitest";
import { canonicalGraph, fanLayout, findHubs, hubSize, type HubOptions } from "@/lib/graph/hubs";
import { clearanceRadius, separateDiscs, sizeRefresh } from "@/lib/graph/clearance";
import { layOut } from "@/lib/graph/layout";
import { louvainGroups, type IndexGraph } from "@/lib/graph/clusters";
import { spineLayout, type Place } from "@/lib/graph/spine-layout";
import { readLook, writeLook } from "@/lib/view-url";
import { buildGraph, indexGraph } from "@/lib/graph/build";
import type { NodeRow, Snapshot } from "@/lib/types";

function graph(n: number, rows: [number, number, number?, number?][]): IndexGraph {
  return {
    n, edges: Uint32Array.from(rows.flatMap(([a, b]) => [a, b])),
    partOf: Uint8Array.from(rows.map((r) => r[2] ?? 0)),
    similarity: Float32Array.from(rows.map((r) => r[3] ?? 0)),
  };
}
const ids = (n: number) => Array.from({ length: n }, (_, i) => `g/${String(i).padStart(5, "0")}`);
const options = (mode: HubOptions["mode"], degree: HubOptions["degree"] = "connections", count = 10): HubOptions => ({ mode, degree, count });
afterEach(() => vi.useRealTimers());
function clear(positions: Float32Array, radii: readonly number[]) {
  for (let i = 0; i < radii.length; i++) for (let j = 0; j < i; j++) {
    if (Math.hypot(positions[i * 2] - positions[j * 2], positions[i * 2 + 1] - positions[j * 2 + 1]) < (radii[i] + radii[j]) * 0.99999) {
      throw new Error(`overlap: ${i}, ${j}`);
    }
  }
}

describe("hub selection", () => {
  it("resolves every named/type declaration and incoming relation, without a count or degree threshold", () => {
    const snapshot: Snapshot = {
      graph: "g", graphId: "gid",
      nodes: [["reg", "register"], ...Array.from({ length: 13 }, (_, i) => [`m${String(i + 1).padStart(2, "0")}`, "meeting"]), ["a", "risk"], ["b", "risk"], ["c", "risk"], ["wrong", "risk"], ["look", "gathering"]]
        .map(([id, type]): NodeRow => [id, type, id, "open"]),
      edges: [
        ["manual", "part_of", "a", "reg"], ["auto", "part_of", "b", "reg", "risk_register"],
        ["shared", "raised_in", "a", "m01"], ["custom", "tracks", "c", "reg"],
        ["wrong-via", "part_of", "wrong", "m02", "meetings"], ["reversed", "raised_in", "m02", "wrong"],
        ["look1", "part_of", "wrong", "look"], ["look2", "part_of", "c", "look"],
      ],
      hubs: [
        { name: "risk_register", form: "named", via: "part_of", node: "reg" },
        { name: "extra_relation", form: "named", via: "tracks", node: "reg" },
        { name: "meetings", form: "type", via: "raised_in", type: "meeting" },
        { name: "gone", form: "named", via: "part_of", node: "missing" },
      ],
      totalNodes: 19, totalEdges: 8, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
    };
    const built = buildGraph(snapshot);
    const indexed = indexGraph(built.graph, undefined, [snapshot]);
    const plan = findHubs(indexed, built.ids, options("declared", "connections", 1), 1, 7);
    const at = (id: string) => built.ids.indexOf(`g/${id}`);
    expect(plan.centres).toHaveLength(14);
    expect(plan.centres).toContain(at("reg"));
    expect(plan.centres).toContain(at("m13"));
    expect(plan.owner[at("a")]).toBe(at("m01"));
    expect(plan.owner[at("b")]).toBe(at("reg"));
    expect(plan.owner[at("c")]).toBe(at("reg"));
    expect(plan.owner[at("wrong")]).toBe(-1);
    expect(plan.owner[at("look")]).toBe(-1);
    expect(built.graph.size).toBe(snapshot.edges.length);
    expect(plan.preview).toBe(false);
    const mask = new Uint8Array(indexed.n).fill(1), edges = new Uint8Array(built.graph.size).fill(1);
    const canonical = canonicalGraph(indexed, built.ids, mask, edges);
    const reversed = buildGraph({ ...snapshot, nodes: [...snapshot.nodes].reverse(), edges: [...snapshot.edges].reverse() });
    const again = canonicalGraph(indexGraph(reversed.graph, undefined, [snapshot]), reversed.ids, mask, edges);
    expect(again.ids).toEqual(canonical.ids);
    expect(again.graph).toEqual(canonical.graph);
    expect(findHubs(indexGraph(built.graph, undefined, [{ graph: "g", hubs: [] }]), built.ids, options("declared"), 1, 7)).toMatchObject({ centres: [], preview: false });
    const legacy = findHubs(indexGraph(built.graph, undefined, [{ graph: "g" }]), built.ids, options("declared"), 1, 7);
    expect(legacy.preview).toBe(true);
    expect(legacy.centres).toContain(at("look"));
  });

  it("scopes declarations to their own graph, including equal ids and types in another open graph", () => {
    const snap = (name: string): Snapshot => ({
      graph: name, graphId: name, nodes: [["hub", "meeting", "Meeting", "open"], ["member", "risk", "Risk", "open"]],
      edges: [["e", "raised_in", "member", "hub"]],
      hubs: name === "a" ? [{ name: "meetings", form: "type", type: "meeting", via: "raised_in" }] : [],
      totalNodes: 2, totalEdges: 1, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
    });
    const snaps = [snap("a"), snap("b")], built = buildGraph(snaps);
    const plan = findHubs(indexGraph(built.graph, undefined, snaps), built.ids, options("declared"), 1, 7);
    expect(plan.centres.map((i) => built.ids[i])).toEqual(["a/hub"]);
    expect(plan.owner[built.ids.indexOf("b/member")]).toBe(-1);
  });

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
  it("does not reprocess pan-only frames and debounces zoom/resize projection changes", () => {
    vi.useFakeTimers();
    let projection: readonly [number, number] = [1, 1];
    const refresh = vi.fn();
    const sizes = sizeRefresh(() => projection, refresh);
    for (let i = 0; i < 100; i++) sizes.changed();
    vi.advanceTimersByTime(500);
    expect(refresh).not.toHaveBeenCalled();
    for (let i = 1; i <= 10; i++) {
      projection = [1 + i / 10, 1];
      sizes.changed();
      vi.advanceTimersByTime(10);
    }
    expect(refresh).not.toHaveBeenCalled();
    for (let i = 0; i < 100; i++) sizes.changed();
    vi.advanceTimersByTime(80);
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(sizes.value()).toEqual([2, 1]);
    projection = [3, 1];
    sizes.changed();
    projection = [2, 1];
    sizes.changed();
    vi.advanceTimersByTime(80);
    expect(refresh).toHaveBeenCalledTimes(1);
    projection = [4, 1];
    sizes.changed();
    sizes.dispose();
    vi.advanceTimersByTime(80);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

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

  it("projects fan arcs into members' own bands, fixes every anchor and keeps unplaced hubs on the shelf", () => {
    const n = 120, g = graph(n, Array.from({ length: n - 4 }, (_, i): [number, number] => [i % 4, i + 4]));
    const plan = findHubs(g, ids(n), options("degree"), 1, 7);
    const sizes = new Float32Array(n).fill(4);
    for (const head of plan.centres) sizes[head] = hubSize(sizes[head]);
    const free = fanLayout(new Float32Array(n * 2), sizes, plan, 1.6);
    const nodes = ids(n).map((key, i) => ({ key, id: String(i), x: free[i * 2], y: free[i * 2 + 1], clearance: clearanceRadius(sizes[i], 1.6) }));
    const owners = new Map([...plan.owner].map((head, i) => [nodes[i].key, nodes[head].key]));
    for (const name of ["first", "second"]) {
      const places = new Map<string, Place>(nodes.map((node, i) => [node.id, { band: i >= 110 || i === 3 ? null : name === "first" ? i % 3 : (i * 7) % 4, by: "field", anchor: i < 3 || i === 4 }]));
      for (const spread of [1, 4]) {
        const normal = spineLayout("g", nodes, places, spread, 1.8);
        const result = spineLayout("g", nodes, places, spread, 1.8, owners, 1.6, true);
        expect(result.axis).toEqual(normal.axis);
        expect(result.at.size).toBe(n);
        for (const node of nodes) if (places.get(node.id)!.anchor && places.get(node.id)!.band !== null) expect(result.at.get(node.key)!.y).toBe(result.axis!.y);
        const bandXs = [...new Set([...places.values()].map((p) => p.band).filter((b): b is number => b !== null))].sort((a, b) => a - b)
          .map((band) => nodes.filter((node) => places.get(node.id)!.band === band).map((node) => result.at.get(node.key)!.x));
        for (let b = 1; b < bandXs.length; b++) expect(Math.max(...bandXs[b - 1])).toBeLessThan(Math.min(...bandXs[b]));
        const placed = nodes.filter((node) => places.get(node.id)!.band !== null);
        const bottom = Math.min(...placed.map((node) => result.at.get(node.key)!.y - result.radii!.get(node.key)!));
        for (const node of nodes) if (places.get(node.id)!.band === null) expect(result.at.get(node.key)!.y + result.radii!.get(node.key)!).toBeLessThan(bottom);
        clear(Float32Array.from(nodes.flatMap((node) => { const p = result.at.get(node.key)!; return [p.x, p.y]; })), nodes.map((node) => result.radii!.get(node.key)!));
        expect(result.at).toEqual(spineLayout("g", [...nodes].reverse(), places, spread, 1.8, owners, 1.6, true).at);
        expect(result.at).not.toEqual(normal.at);
      }
    }
    expect(free).toEqual(fanLayout(new Float32Array(n * 2), sizes, plan, 1.6));
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

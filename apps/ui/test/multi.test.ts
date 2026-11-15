// Several graphs on one canvas (ws009 M1a, plan 133): grouping and colour by graph, each graph seeded in a region of
// its own, focus across the links between open graphs, search merged across graphs, and umx's hints (D69).
import { describe, expect, it } from "vitest";
import { hintItems } from "@/components/hit-list";
import { mergeResults } from "@/components/use-search";
import { applyLook, applyVisibility, buildGraph, graphGroups, indexGraph, shownMask } from "@/lib/graph/build";
import { louvainGroups } from "@/lib/graph/clusters";
import { RETIRED_STATUSES } from "@/lib/graph/colors";
import { hiddenReasons, NO_FILTERS, reasonText } from "@/lib/graph/filters";
import { focusDepths } from "@/lib/graph/focus";
import { graphColor } from "@/lib/graph/graph-colors";
import { qid } from "@/lib/graph/keys";
import { runLayout, seedPositions } from "@/lib/graph/layout";
import { SPACING } from "@/lib/graph/spacing";
import type { IndexGraph } from "@/lib/graph/clusters";
import type { EdgeRow, ExternalRow, Hit, NodeRow, SearchResult, Snapshot } from "@/lib/types";

const id = (i: number) => `01${String(i).padStart(24, "0")}`;
const [GA, GB] = ["01GRAPHAAAAAAAAAAAAAAAAAAA", "01GRAPHBBBBBBBBBBBBBBBBBBB"];
const HIDE = { provenance: false, retired: false };

/** A chain of `n` nodes in one graph, ids 0..n-1 (the same ids in every graph). */
function chain(graph: string, graphId: string, n: number, external: ExternalRow[] = []): Snapshot {
  const nodes: NodeRow[] = Array.from({ length: n }, (_v, i) => [id(i), "risk", `${graph} ${i}`, "open"]);
  const edges: EdgeRow[] = Array.from({ length: n - 1 }, (_v, i) => [id(100 + i), "supports", id(i), id(i + 1)]);
  return { graph, graphId, nodes, edges, external, totalNodes: n, totalEdges: n - 1, truncated: false, omittedEdges: { otherGraph: external.length, unresolved: 0 } };
}

function two() {
  // alpha's last node links to beta's first.
  const alpha = chain("alpha", GA, 6, [[id(900), "supports", GA, id(5), GB, id(0), false]]);
  const beta = chain("beta", GB, 4);
  const built = buildGraph([alpha, beta]);
  const ranks = new Map([["alpha", 0], ["beta", 1]]);
  return { ...built, ranks };
}

describe("group and colour by graph", () => {
  it("groups by graph in open order, leaves hidden nodes out, and colours each graph its own hue", () => {
    const { graph, ranks } = two();
    graph.mergeNodeAttributes(qid("beta", id(3)), { status: "retired" });
    applyVisibility(graph, HIDE);
    const groups = graphGroups(graph, ranks, shownMask(graph, HIDE));
    expect(groups.sizes).toEqual([6, 3]);
    expect(groups.of[graph.getNodeAttribute(qid("beta", id(3)), "index")]).toBe(-1);
    applyLook(graph, "light", "graph", groups, ranks);
    expect(graph.getNodeAttribute(qid("alpha", id(0)), "color")).toBe(graphColor(0, "light"));
    expect(graph.getNodeAttribute(qid("beta", id(0)), "color")).toBe(graphColor(1, "light"));
    expect(graphColor(0, "dark")).not.toBe(graphColor(1, "dark"));
  });

  it("seeds each graph in a region of its own; one graph seeds exactly as before", () => {
    const { graph, ranks } = two();
    const mask = shownMask(graph, HIDE);
    const one = indexGraph(graph);
    expect(one.part).toBeUndefined();
    const parted = indexGraph(graph, ranks);
    const groups = louvainGroups(parted, mask, 1, 7);
    const p = seedPositions(parted, groups, 7);
    const centre = (r: number) => {
      const xs = [...parted.part!].map((v, i) => (v === r ? [p[i * 2], p[i * 2 + 1]] : null)).filter((v): v is number[] => v !== null);
      return [xs.reduce((s, v) => s + v[0], 0) / xs.length, xs.reduce((s, v) => s + v[1], 0) / xs.length];
    };
    const [a, b] = [centre(0), centre(1)];
    expect(Math.hypot(a[0] - b[0], a[1] - b[1])).toBeGreaterThan(5);
    // Every node in one part seeds exactly as a graph with no parts (a one-graph view is unchanged).
    expect([...seedPositions({ ...one, part: new Int32Array(one.n) }, groups, 7)]).toEqual([...seedPositions(one, groups, 7)]);
  });
});

describe("layout of several graphs", () => {
  it("keeps each graph together even when only external edges join its parts", () => {
    // Two graphs of four separate 10-node stars each; star k of graph 0 links to star k of graph 1.
    const edges: number[] = [];
    const part = new Int32Array(80);
    for (let g = 0; g < 2; g++) {
      for (let k = 0; k < 4; k++) {
        const hub = g * 40 + k * 10;
        for (let j = 1; j < 10; j++) edges.push(hub, hub + j);
        for (let j = 0; j < 10; j++) part[hub + j] = g;
      }
    }
    for (let k = 0; k < 4; k++) edges.push(k * 10, 40 + k * 10);
    const g: IndexGraph = { n: 80, edges: Uint32Array.from(edges), partOf: new Uint8Array(edges.length / 2), part };
    const mask = new Uint8Array(80).fill(1);
    const spread = (pos: Float32Array) => {
      const c = [0, 1].map((p) => {
        const idx = [...part.keys()].filter((i) => part[i] === p);
        return [idx.reduce((s, i) => s + pos[i * 2], 0) / idx.length, idx.reduce((s, i) => s + pos[i * 2 + 1], 0) / idx.length, idx] as const;
      });
      const within = c.flatMap(([x, y, idx]) => idx.map((i) => Math.hypot(pos[i * 2] - x, pos[i * 2 + 1] - y)));
      return { within: within.reduce((s, v) => s + v, 0) / within.length, between: Math.hypot(c[0][0] - c[1][0], c[0][1] - c[1][1]) };
    };
    const laid = (ig: IndexGraph) => runLayout(ig, new Float32Array(80).fill(2), seedPositions(ig, louvainGroups(ig, mask, 1, 3), 3), 60_000, SPACING.default).positions;
    const anchored = spread(laid(g));
    expect(anchored.between).toBeGreaterThan(anchored.within);
    expect(laid(g)).toHaveLength(160);
  });
});

describe("focus across open graphs", () => {
  it("walks across an external edge into the other open graph, by graph-qualified keys", () => {
    const { graph } = two();
    applyVisibility(graph, HIDE);
    const near = focusDepths(graph, qid("alpha", id(5)), 2);
    expect(near.get(qid("beta", id(0)))).toBe(1);
    expect(near.get(qid("beta", id(1)))).toBe(2);
    // The same id in alpha is another node, two hops the other way.
    expect(near.get(qid("alpha", id(3)))).toBe(2);
    expect(near.has(qid("alpha", id(0)))).toBe(false);
  });
});

describe("search merged across graphs", () => {
  const hit = (n: number, title: string): Hit => ({ id: id(n), type: "risk", title, status: "open", score: 1 / (n + 1), legs: null, exact: null });
  const result = (hits: Hit[], warnings = [] as SearchResult["warnings"]): SearchResult => ({ mode: "title", query: "q", hits, warnings, note: null, foreign: 0 });

  it("keys each hit by its graph, takes turns rank by rank, and says which graph a warning came from", () => {
    const merged = mergeResults(
      [
        { alias: "alpha", result: result([hit(0, "a0"), hit(1, "a1"), hit(2, "a2")], [{ code: "similarity_unavailable", message: "no vectors", next: null }]) },
        { alias: "beta", result: result([hit(0, "b0")]) },
      ],
      true,
    );
    expect(merged.hits.map((h) => [h.id, h.graph])).toEqual([
      [qid("alpha", id(0)), "alpha"],
      [qid("beta", id(0)), "beta"],
      [qid("alpha", id(1)), "alpha"],
      [qid("alpha", id(2)), "alpha"],
    ]);
    expect(merged.warnings[0].message).toBe("alpha: no vectors");
    // One graph: keys, but no graph label and the warning as given.
    const single = mergeResults([{ alias: "alpha", result: result([hit(0, "a0")], [{ code: "w", message: "m", next: null }]) }], false);
    expect(single.hits[0]).toMatchObject({ id: qid("alpha", id(0)) });
    expect(single.hits[0].graph).toBeUndefined();
    expect(single.warnings[0].message).toBe("m");
  });

  it("folds the graphs that found nothing into one quiet line while another graph has hits (ui-expert N2)", () => {
    const none = { code: "find_no_match", message: "No title matched.", next: "umx docs get search" };
    const parts = [
      { alias: "alpha", result: result([hit(0, "a0")]) },
      { alias: "beta", result: result([], [none]) },
      { alias: "gamma", result: result([], [none]) },
    ];
    const merged = mergeResults(parts, true);
    expect(merged.warnings).toEqual([]);
    expect(merged.quiet).toBe("No title match in beta, gamma");
    // Nothing found anywhere: every graph's warning, as before.
    const empty = mergeResults(parts.slice(1), true);
    expect(empty.warnings).toHaveLength(2);
    expect(empty.quiet).toBeUndefined();
  });

  it("folds them the same when umx says find_no_match as a hint (protocol 70, ws002 X1; the project manager)", () => {
    const none = { code: "find_no_match", message: "No title matched.", next: "umx docs get search" };
    const hinted = (hits: Hit[], hints: SearchResult["warnings"]): SearchResult => ({ ...result(hits), hints });
    const parts = [
      { alias: "alpha", result: hinted([hit(0, "a0")], []) },
      { alias: "beta", result: hinted([], [none]) },
      { alias: "gamma", result: hinted([], [none]) },
    ];
    const merged = mergeResults(parts, true);
    expect(merged.quiet).toBe("No title match in beta, gamma");
    expect(merged.warnings).toEqual([]);
    expect(merged.hints).toEqual([]);
    // A graph with another hint beside find_no_match is not folded, and its hints stay.
    const other = { code: "graph_loaded_on_demand", message: "loaded", next: "umx graph list" };
    const kept = mergeResults([parts[0], parts[1], { alias: "gamma", result: hinted([], [none, other]) }], true);
    expect(kept.quiet).toBe("No title match in beta");
    expect(kept.hints?.map((h) => h.message)).toEqual(["gamma: No title matched.", "gamma: loaded"]);
  });
});

describe("umx's hints and out-of-play nodes (D69)", () => {
  it("lists umx's hints and every warning that names a next command, each once with a count, inventing none", () => {
    const items = hintItems(
      [
        { code: "edge_broken", message: "an end is gone", next: "umx check --graph alpha" },
        { code: "edge_broken", message: "an end is gone", next: "umx check --graph alpha" },
        { code: "similarity_unavailable", message: "no vectors", next: null },
      ],
      [{ code: "hint", message: "Run umx graph guide alpha first", next: "umx graph guide alpha" }],
    );
    expect(items.map((h) => [h.code, h.count])).toEqual([["hint", 1], ["edge_broken", 2]]);
    expect(hintItems([{ code: "w", message: "m", next: null }])).toEqual([]);
  });

  it("hides retired, superseded and ruled_out nodes behind the one out-of-play toggle", () => {
    expect([...RETIRED_STATUSES].sort()).toEqual(["retired", "ruled_out", "superseded"]);
    const node = { kind: "risk", status: "superseded" };
    expect(hiddenReasons(node, HIDE, NO_FILTERS)).toEqual(["retired"]);
    expect(reasonText(node, "retired")).toBe("out of play");
    expect(hiddenReasons(node, { provenance: false, retired: true }, NO_FILTERS)).toEqual([]);
  });
});

// Live changes applied to the graph on screen in place (lib/graph/apply.ts, plan 127).
import { describe, expect, it } from "vitest";
import { applyNudge, applyRows, applySnapshot, changeText, extentOf, goneAfterRead, nudgeInput, qualifyRows, remapGroups, reopen, snapAfter } from "@/lib/graph/apply";
import { buildGraph, chipCounts, counts, indexGraph, neighbourLinks, openByGraphId, shownMask } from "@/lib/graph/build";
import { NO_FILTERS } from "@/lib/graph/filters";
import { qid } from "@/lib/graph/keys";
import { nudge } from "@/lib/graph/layout";
import type { EdgeRow, ExternalRow, Neighbour, NodeRow, Snapshot } from "@/lib/types";

const id = (c: string) => `01${c.repeat(24)}`;
const [rA, rB, rC, rD, rX] = ["A", "B", "C", "D", "X"].map(id);
const [rE1, rE2, rE3] = ["E", "F", "G"].map((c) => `01EDGE${c.repeat(20)}`);
// Canvas keys: rows reach applyRows qualified by their graph (plan 133).
const k = (raw: string) => qid("demo", raw);
const [A, B, C, D, X] = [rA, rB, rC, rD, rX].map(k);
const [E1, E2, E3] = [rE1, rE2, rE3].map(k);
const NOTHING = { nodes: [] as string[], edges: [] as string[] };

function snap(nodes: NodeRow[], edges: EdgeRow[], graph = "demo", graphId = id("Z"), external: ExternalRow[] = []): Snapshot {
  return { graph, graphId, nodes, edges, external, totalNodes: nodes.length, totalEdges: edges.length, truncated: false, omittedEdges: { otherGraph: external.length, unresolved: 0 } };
}

function base() {
  const built = buildGraph(
    snap(
      [[rA, "problem", "Alpha", "open"], [rB, "option", "Beta", "open"], [rC, "risk", "Gamma", "open"]],
      [[rE1, "addresses", rB, rA], [rE2, "threatens", rC, rB]],
    ),
  );
  const g = built.graph;
  g.updateEachNodeAttributes((n, a) => ({ ...a, x: a.index * 10, y: 0 }));
  return g;
}

const dense = (g: ReturnType<typeof base>) => {
  const seen: number[] = [];
  g.forEachNode((_n, a) => seen.push(a.index));
  return seen;
};

describe("applying rows in place", () => {
  it("preserves attribution through qualification and updates/removes it without moving nodes", () => {
    const g = base(), before = g.mapNodes((key, a) => [key, a.x, a.y]);
    const row = qualifyRows("demo", { nodes: [], edges: [[rE1, "addresses", rB, rA, "register"]] }, new Set(["demo"]));
    const out = applyRows(g, row, NOTHING);
    expect(out.edges.edited).toBe(1);
    expect(g.getEdgeAttribute(E1, "autoAddedBy")).toBe("register");
    const n: Neighbour = { kind: "node", way: "in", edge: rE1, relation: "addresses", id: rB, graph: id("Z"), graphAlias: "demo", local: true, type: "option", title: "Beta", status: "open" };
    const links = () => neighbourLinks(new Map([[id("Z"), "demo"]]), "demo", new Set(), { graph: g, node: A });
    expect(links().edgeOrigin(n)).toBe("register");
    applyRows(g, { nodes: [], edges: [[E1, "addresses", B, A]] }, NOTHING);
    expect(links().edgeOrigin(n)).toBeNull();
    expect(g.mapNodes((key, a) => [key, a.x, a.y])).toEqual(before);
    expect(qualifyRows("demo", { nodes: [], edges: [], external: [[rE3, "part_of", "demo", rA, "other", rB, "register"]] }, new Set(["demo", "other"])).edges[0][4]).toBe("register");
  });

  it("keeps declaration metadata current on empty live replies, including an authoritative empty list", () => {
    const initial = snap([[rA, "meeting", "Meeting", "open"]], []);
    const g = buildGraph(initial).graph;
    const outcome = applyRows(g, { nodes: [], edges: [] }, NOTHING);
    const declared = snapAfter(initial, outcome, { hubs: [{ name: "meetings", form: "type", via: "raised_in", type: "meeting" }] });
    expect([...indexGraph(g, undefined, [declared]).declared!]).toEqual([1]);
    const cleared = snapAfter(declared, outcome, { hubs: [] });
    expect([...indexGraph(g, undefined, [cleared]).declared!]).toEqual([0]);
    expect(snapAfter(declared, outcome, {}).hubs).toEqual(declared.hubs);
  });

  it("adds, edits and removes nodes and edges on the same graph instance, renumbering nodes densely", () => {
    const g = base();
    const out = applyRows(
      g,
      { nodes: [[D, "option", "Delta", "proposed"], [A, "problem", "Alpha (renamed)", "agreed"]], edges: [[E3, "addresses", D, A]] },
      { nodes: [C], edges: [] },
    );
    expect(g.hasNode(D) && g.hasEdge(E3)).toBe(true);
    expect(g.getNodeAttributes(A)).toMatchObject({ label: "Alpha (renamed)", status: "agreed" });
    expect(g.hasNode(C) || g.hasEdge(E2)).toBe(false);
    expect(out.nodes).toEqual({ added: 1, edited: 1, removed: 1 });
    expect(out.edges).toEqual({ added: 1, edited: 0, removed: 1 });
    expect(out.fresh).toEqual([D]);
    expect(out.removed).toEqual([C]);
    expect(dense(g)).toEqual([0, 1, 2]);
    expect(out.ids).toEqual([A, B, D]);
    expect(g.getNodeAttribute(A, "degree")).toBe(2);
    expect(indexGraph(g).n).toBe(3);
  });

  it("changes nothing for a row equal to what is drawn", () => {
    const g = base();
    const out = applyRows(g, { nodes: [[A, "problem", "Alpha", "open"]], edges: [[E1, "addresses", B, A]] }, NOTHING);
    expect(changeText(out)).toBe("");
    expect(out.touched.size).toBe(0);
  });

  it("re-adds an edge whose relation or direction changed, and drops one whose end is not on the canvas", () => {
    const g = base();
    const out = applyRows(g, { nodes: [], edges: [[E1, "addresses", A, B], [E2, "threatens", C, X]] }, NOTHING);
    expect([g.source(E1), g.target(E1)]).toEqual([A, B]);
    expect(g.hasEdge(E2)).toBe(false);
    expect(out.edges).toEqual({ added: 0, edited: 1, removed: 1 });
  });

  it("removing an unknown id (one the view does not hold) is a silent no-op: nothing counted, nothing touched", () => {
    const g = base();
    const before = { order: g.order, size: g.size };
    const out = applyRows(g, { nodes: [], edges: [] }, { nodes: [X, k(id("P"))], edges: [k(`01EDGE${"Q".repeat(20)}`)] });
    expect({ order: g.order, size: g.size }).toEqual(before);
    expect(out.nodes).toEqual({ added: 0, edited: 0, removed: 0 });
    expect(out.edges).toEqual({ added: 0, edited: 0, removed: 0 });
    expect(out.touched.size).toBe(0);
    expect(changeText(out)).toBe("");
  });

  it("treats an id asked for and not returned as gone, beside the ones the change removed", () => {
    const asked = { nodes: [A, D], edges: [E1] };
    const got = { nodes: [[A, "problem", "Alpha", "open"]] as NodeRow[], edges: [] as EdgeRow[] };
    expect(goneAfterRead(asked, got, { nodes: [C], edges: [E2] })).toEqual({ nodes: [C, D], edges: [E2, E1] });
  });

  it("places a new node next to its neighbours", () => {
    const g = base();
    applyRows(g, { nodes: [[D, "option", "Delta", "open"]], edges: [[E3, "addresses", D, A]] }, NOTHING);
    const a = g.getNodeAttributes(A);
    const d = g.getNodeAttributes(D);
    expect(Math.hypot(d.x - a.x, d.y - a.y)).toBeLessThan(20);
  });

  it("filters and counts follow the new data", () => {
    const g = base();
    const filters = { ...NO_FILTERS, types: new Set(["option"]) };
    applyRows(g, { nodes: [[D, "option", "Delta", "open"], [X, "decision", "Xi", "open"]], edges: [] }, { nodes: [C], edges: [] });
    const visible = { provenance: false, retired: false };
    const chips = chipCounts(g, visible, filters);
    expect(chips.types.get("option")).toBe(2);
    expect(chips.types.get("decision")).toBe(1);
    expect(chips.types.has("risk")).toBe(false);
    const mask = shownMask(g, visible, filters);
    expect(mask[g.getNodeAttribute(D, "index")]).toBe(0);
    expect(mask[g.getNodeAttribute(X, "index")]).toBe(1);
    expect(counts(g).types.get("decision")).toBe(1);
  });
});

describe("overflow: the snapshot again, applied against what is drawn", () => {
  it("keeps positions of the nodes that stay and drops what is gone", () => {
    const g = base();
    const bx = g.getNodeAttribute(B, "x");
    const out = applySnapshot(g, snap([[rA, "problem", "Alpha", "open"], [rB, "option", "Beta", "open"], [rD, "risk", "Delta", "open"]], [[rE1, "addresses", rB, rA]]), new Map());
    expect(g.getNodeAttribute(B, "x")).toBe(bx);
    expect(out.nodes).toEqual({ added: 1, edited: 0, removed: 1 });
    expect(out.edges.removed).toBe(1);
    expect(g.hasEdge(E2)).toBe(false);
  });
});

describe("several graphs on one canvas (plan 133)", () => {
  const [GA, GB] = [id("1"), id("2")];
  const [xE1, xE2] = ["H", "J"].map((c) => `01EDGE${c.repeat(20)}`);
  // alpha and beta hold a node with the SAME id (rA), and alpha stores two edges into beta.
  const alpha = () =>
    snap([[rA, "problem", "Alpha A", "open"], [rB, "option", "Alpha B", "open"]], [[rE1, "addresses", rB, rA]], "alpha", GA, [
      [xE1, "supports", GA, rA, GB, rA, false],
      [xE2, "supports", GA, rB, GB, rC, false],
    ]);
  const beta = () => snap([[rA, "risk", "Beta A", "open"], [rC, "risk", "Beta C", "open"]], [[rE1, "threatens", rC, rA]], "beta", GB);

  it("rereads one graph on overflow and leaves the other graph's nodes, edges and links alone", () => {
    const g = buildGraph([alpha(), beta()]).graph;
    expect(g.size).toBe(4);
    const open = openByGraphId([alpha(), beta()]);
    // beta loses C: its own edge and alpha's link into it go with it; alpha and its other link stay.
    const out = applySnapshot(g, snap([[rA, "risk", "Beta A", "open"]], [], "beta", GB), open);
    expect(g.hasNode(qid("beta", rC))).toBe(false);
    expect(g.hasNode(qid("alpha", rA)) && g.hasNode(qid("alpha", rB))).toBe(true);
    expect(g.hasEdge(qid("alpha", rE1)) && g.hasEdge(qid("alpha", xE1))).toBe(true);
    expect(g.hasEdge(qid("beta", rE1)) || g.hasEdge(qid("alpha", xE2))).toBe(false);
    expect(out.nodes.removed).toBe(1);
    // alpha's overflow brings its link back once beta has C again.
    applyRows(g, { nodes: [[qid("beta", rC), "risk", "Beta C", "open"]], edges: [] }, NOTHING);
    const again = applySnapshot(g, alpha(), open);
    expect(g.hasEdge(qid("alpha", xE2))).toBe(true);
    expect(again.edges.added).toBe(1);
    expect(g.getNodeAttribute(qid("beta", rC), "graph")).toBe("beta");
    // An external edge whose relation and direction changed while the tab overflowed is re-added as it is now.
    const turned = alpha();
    turned.external = [[xE1, "blocks", GB, rA, GA, rA, false], turned.external![1]];
    const edited = applySnapshot(g, turned, open);
    const key = qid("alpha", xE1);
    expect([g.getEdgeAttribute(key, "kind"), g.source(key), g.target(key)]).toEqual(["blocks", qid("beta", rA), qid("alpha", rA)]);
    expect(edited.edges.edited).toBe(1);
  });

  it("qualifies a live read's rows by graph, and keeps an external edge only when both its graphs are open", () => {
    const rows = {
      nodes: [[rD, "option", "D", "open"]] as NodeRow[],
      edges: [[rE3, "addresses", rD, rA]] as EdgeRow[],
      external: [
        [xE1, "supports", "alpha", rD, "beta", rA],
        [xE2, "supports", "alpha", rD, "gamma", rA],
      ] as [string, string, string, string, string, string][],
    };
    expect(qualifyRows("alpha", rows, new Set(["alpha", "beta"]))).toEqual({
      nodes: [[qid("alpha", rD), "option", "D", "open"]],
      edges: [
        [qid("alpha", rE3), "addresses", qid("alpha", rD), qid("alpha", rA)],
        [qid("alpha", xE1), "supports", qid("alpha", rD), qid("beta", rA)],
      ],
    });
  });
});

describe("groups across a renumbering, and the nudge", () => {
  it("keeps each node's group by id, puts new nodes in none, and moves a head that left", () => {
    const groups = { of: Int32Array.from([0, 0, 1]), sizes: [2, 1], heads: [0, 2] };
    const r = remapGroups(groups, [A, B, C], [B, D, A]);
    expect([...r.of]).toEqual([0, -1, 0]);
    expect(r.sizes).toEqual([2, 0]);
    expect(r.heads).toEqual([2, -1]);
  });

  it("moves only the new nodes; the nodes around them stay where they were", () => {
    const g = base();
    applyRows(g, { nodes: [[D, "option", "Delta", "open"]], edges: [[E3, "addresses", D, A]] }, NOTHING);
    const made = nudgeInput(g, [D])!;
    expect(made.ids[0]).toBe(D);
    expect(made.input.fixed[0]).toBe(0);
    expect([...made.input.fixed].slice(1).every((f) => f === 1)).toBe(true);
    const before = new Map(made.ids.map((n) => [n, [g.getNodeAttribute(n, "x"), g.getNodeAttribute(n, "y")]]));
    const positions = nudge(made.input);
    applyNudge(g, made.ids, made.input.fixed, positions);
    for (const n of made.ids.slice(1)) expect([g.getNodeAttribute(n, "x"), g.getNodeAttribute(n, "y")]).toEqual(before.get(n));
    expect(Number.isFinite(g.getNodeAttribute(D, "x"))).toBe(true);
  });
});

describe("the indicator's words", () => {
  it("says what changed on the canvas", () => {
    const c = (added: number, edited: number, removed: number) => ({ added, edited, removed });
    expect(changeText({ nodes: c(3, 0, 0), edges: c(0, 0, 1) })).toBe("+3 nodes, 1 edge removed");
    expect(changeText({ nodes: c(0, 1, 0), edges: c(2, 0, 0) })).toBe("+2 edges, 1 node changed");
  });
});

describe("another open set from the canvas (ws009 M1b1)", () => {
  const [ga, gb, gc] = [id("1"), id("2"), id("3")];
  const [a1, a2, b1, c1] = ["H", "I", "J", "K"].map(id);
  const [ea, eb, ei] = ["L", "M", "N"].map((c) => `01EDGE${c.repeat(20)}`);
  const alpha = snap([[a1, "problem", "A one", "open"], [a2, "option", "A two", "open"]], [[ei, "addresses", a2, a1]], "alpha", ga, [[ea, "relates_to", ga, a1, gb, b1, false]]);
  const beta = snap([[b1, "risk", "B one", "open"]], [], "beta", gb, [[eb, "threatens", gb, b1, ga, a2, false]]);
  const gamma = snap([[c1, "risk", "C one", "open"]], [], "gamma", gc);
  const prev = () => {
    const g = buildGraph([alpha, gamma]).graph;
    g.updateEachNodeAttributes((_n, a) => ({ ...a, x: a.index * 10, y: 5 }));
    // A live change the snapshot does not have: the kept graph keeps it.
    g.mergeNodeAttributes(qid("alpha", a1), { label: "A one, edited live" });
    return g;
  };

  it("keeps the kept graphs as drawn, drops the closed ones, and adds the new ones with the edges between", () => {
    const before = prev();
    const r = reopen(before, ["alpha", "beta"], [alpha], [beta]);
    expect(r.graph.nodes().sort()).toEqual([qid("alpha", a1), qid("alpha", a2), qid("beta", b1)].sort());
    expect(r.graph.getNodeAttributes(qid("alpha", a1))).toMatchObject({ label: "A one, edited live", x: 0, y: 5 });
    expect(r.graph.hasEdge(qid("alpha", ea)) && r.graph.hasEdge(qid("beta", eb)) && r.graph.hasEdge(qid("alpha", ei))).toBe(true);
    expect(r.fresh).toEqual([qid("beta", b1)]);
    expect(r.ids.map((n) => r.graph.getNodeAttribute(n, "index"))).toEqual([0, 1, 2]);
    expect([...r.omitted.keys()]).toEqual(["alpha", "beta"]);
    // The canvas on screen is never touched.
    r.graph.setNodeAttribute(qid("alpha", a2), "x", 999);
    expect(before.hasNode(qid("gamma", c1))).toBe(true);
    expect(before.getNodeAttribute(qid("alpha", a2), "x")).toBe(10);
    expect(before.getNodeAttribute(qid("alpha", a2), "index")).toBe(1);
  });

  it("an Undo closes the added graph and its edges, keeping the rest exactly", () => {
    const opened = reopen(prev(), ["alpha", "beta"], [alpha], [beta]).graph;
    const back = reopen(opened, ["alpha"], [alpha], []);
    expect(back.graph.nodes()).toEqual([qid("alpha", a1), qid("alpha", a2)]);
    expect(back.graph.edges()).toEqual([qid("alpha", ei)]);
    expect(back.omitted.get("alpha")?.get(qid("alpha", ea))).toBe("notOpen");
  });

  it("frames as sigma does: every node's x and y", () => {
    expect(extentOf(prev())).toEqual({ x: [0, 20], y: [5, 5] });
  });
});

// Protocol-25 external endpoints in a one-graph view (review R170-1..3). Rows are shaped like the daemon's
// serializers (crates/umx-daemon/src/verbs.rs, neighbours rows and edge_row).
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { NeighbourList } from "@/components/node-panel";
import { buildGraph } from "@/lib/graph/build";
import { qid } from "@/lib/graph/keys";
import type { Snapshot } from "@/lib/types";
import { loadNode, loadSnapshot } from "@/lib/snapshot";
import { createUmx, UMX_PROTOCOL, type Exec } from "@/lib/umx";

const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA"; // alpha, on screen
const GB = "01GRAPHBBBBBBBBBBBBBBBBBBB"; // beta, loaded
const GC = "01GRAPHCCCCCCCCCCCCCCCCCCC"; // not loaded
const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB"; // the same id names alpha/B and beta/B
const C = "01NODECCCCCCCCCCCCCCCCCCCC";

const env = (data: unknown) => JSON.stringify({ umx: UMX_PROTOCOL, command: "x", status: "ok", data, warnings: [], errors: [] });
const page = (rows: unknown[]) => env({ rows, total: rows.length, truncated: false });
const edge = (id: string, from: string, to: string, fromGraph: string, toGraph: string, extra: object = {}) => ({
  id, type: "supports", from, to, graph: "alpha", graph_id: GA, from_graph: fromGraph, to_graph: toGraph, sentence: "s", ...extra,
});

const ROWS: Record<string, string> = {
  "graph list": page([
    { alias: "alpha", id: GA, nodes: 2, edges: 4, state: "loaded", folder: "present", policy: "open" },
    { alias: "beta", id: GB, nodes: 1, edges: 0, state: "loaded", folder: "present", policy: "open" },
    // Remembered, not loaded: its alias must not name GC's rows as if it were loaded.
    { alias: "gamma", id: GC, nodes: null, edges: null, state: "unloaded", folder: "present", policy: "open" },
  ]),
  "node list": page([
    { id: A, type: "problem", title: "a", status: "open" },
    { id: B, type: "option", title: "alpha b", status: "open" },
  ]),
  "edge list": page([
    edge("01EDGELOCALAAAAAAAAAAAAAAA", A, B, GA, GA, { from_title: "a", to_title: "alpha b" }),
    edge("01EDGEBETAAAAAAAAAAAAAAAAA", A, B, GA, GB, { from_title: "a", to_title: "beta b" }),
    edge("01EDGEINBOUNDAAAAAAAAAAAAA", B, A, GB, GA, { from_title: "beta b", to_title: "a" }),
    edge("01EDGEGONEAAAAAAAAAAAAAAAA", A, C, GA, GC, { from_title: "a", to_state: "graph_not_loaded" }),
  ]),
  "ontology show": env({ symmetric: ["conflicts_with", "rhymes_with"] }),
  "node get": env({ node: { id: A, type: "problem", title: "a", status: "open", graph: "alpha" } }),
  neighbours: page([
    { way: "out", relation: "supports", edge: "e1", id: B, graph: GA, edge_graph: GA, from: A, to: B, from_graph: GA, to_graph: GA, type: "option", title: "alpha b", status: "open" },
    { way: "out", relation: "supports", edge: "e2", id: B, graph: GB, edge_graph: GA, from: A, to: B, from_graph: GA, to_graph: GB, type: "option", title: "beta b", status: "agreed" },
    { way: "out", relation: "supports", edge: "e3", id: C, graph: GC, edge_graph: GA, from: A, to: C, from_graph: GA, to_graph: GC, state: "graph_not_loaded" },
    { way: "in", relation: "raises", edge: "e4", id: C, graph: GB, edge_graph: GB, from: C, to: A, from_graph: GB, to_graph: GA, state: "copy_ambiguous", candidates: ["umx:beta/x", "umx:beta/y"] },
    { way: "out", relation: "blocks", edge: "e5", id: C, graph: GA, edge_graph: GA, from: A, to: C, from_graph: GA, to_graph: GA, state: "edge_broken" },
  ]),
};

/** Each node's row as `node get <id> --graph <alias> --columns ...` gives it (the flagged read neighbour titles come
 * from, ws009 M2b). */
const ROW_OF: Record<string, object> = {
  [`alpha ${B}`]: { id: B, type: "option", title: "alpha b", status: "open" },
  [`beta ${B}`]: { id: B, type: "option", title: "beta b", status: "agreed" },
};

function reader() {
  const exec = vi.fn<Exec>(async (_bin, args) => ({
    stdout: args[1] === "get" && args.includes("--columns") ? env({ node: ROW_OF[`${args[4]} ${args[2]}`] }) : ROWS[args[0] === "neighbours" ? "neighbours" : `${args[0]} ${args[1]}`],
    failed: false,
    detail: "",
  }));
  return { exec, read: createUmx({ exec, bin: () => "/x/umx", env: () => ({ UMX_HOME: "/h", UMX_OUTPUT: "json" }) }) };
}

describe("edges with an end outside the graph on screen (R170-3)", () => {
  it("draws only edges with both ends resolved here and counts the rest by reason", async () => {
    const snap = await loadSnapshot(reader().read, "alpha");
    expect(snap.graphId).toBe(GA);
    expect(snap.edges.map((e) => e[0])).toEqual(["01EDGELOCALAAAAAAAAAAAAAAA"]);
    expect(snap.omittedEdges).toEqual({ otherGraph: 2, unresolved: 1 });
    expect(snap.totalEdges).toBe(4);
    expect(snap.symmetric).toEqual(["conflicts_with", "rhymes_with"]);
  });

  it("never turns alpha/A -> beta/B into alpha/A -> alpha/B, even though alpha has a node B", async () => {
    const { graph, external } = buildGraph(await loadSnapshot(reader().read, "alpha"));
    expect(graph.hasNode(qid("alpha", B))).toBe(true);
    expect(graph.hasEdge(qid("alpha", "01EDGEBETAAAAAAAAAAAAAAAAA"))).toBe(false);
    expect(graph.hasEdge(qid("alpha", "01EDGEINBOUNDAAAAAAAAAAAAA"))).toBe(false);
    expect(graph.hasEdge(qid("alpha", "01EDGEGONEAAAAAAAAAAAAAAAA"))).toBe(false);
    expect(graph.size).toBe(1);
    expect(external).toEqual({ drawn: 0, notOpen: 3, broken: 0 });
  });
});

describe("several graphs on one canvas (plan 133)", () => {
  const beta: Snapshot = {
    graph: "beta",
    graphId: GB,
    nodes: [[B, "option", "beta b", "agreed"]],
    edges: [],
    external: [],
    totalNodes: 1,
    totalEdges: 0,
    truncated: false,
    omittedEdges: { otherGraph: 0, unresolved: 0 },
  };

  it("keys equal ids in two graphs apart and draws an external edge once both its graphs are open", async () => {
    const alpha = await loadSnapshot(reader().read, "alpha");
    expect(alpha.external?.map((e) => [e[0], e[6]])).toEqual([
      ["01EDGEBETAAAAAAAAAAAAAAAAA", false],
      ["01EDGEINBOUNDAAAAAAAAAAAAA", false],
      ["01EDGEGONEAAAAAAAAAAAAAAAA", true],
    ]);
    const { graph, external } = buildGraph([alpha, beta]);
    expect(graph.order).toBe(3);
    expect(graph.getNodeAttribute(qid("alpha", B), "label")).toBe("alpha b");
    expect(graph.getNodeAttribute(qid("beta", B), "label")).toBe("beta b");
    const ends = (e: string) => [graph.source(qid("alpha", e)), graph.target(qid("alpha", e))];
    expect(ends("01EDGEBETAAAAAAAAAAAAAAAAA")).toEqual([qid("alpha", A), qid("beta", B)]);
    expect(ends("01EDGEINBOUNDAAAAAAAAAAAAA")).toEqual([qid("beta", B), qid("alpha", A)]);
    // gamma is not open: its edge stays off and counted, never named.
    expect(graph.hasEdge(qid("alpha", "01EDGEGONEAAAAAAAAAAAAAAAA"))).toBe(false);
    expect(external).toEqual({ drawn: 2, notOpen: 1, broken: 0 });
  });

  it("draws a link into a graph the read gave as not loaded only when that graph is open by name", async () => {
    const alpha = await loadSnapshot(reader().read, "alpha");
    const gamma: Snapshot = { ...beta, graph: "gamma", graphId: GC, nodes: [[C, "risk", "gamma c", "open"]] };
    const { graph, external } = buildGraph([alpha, gamma]);
    expect([graph.source(qid("alpha", "01EDGEGONEAAAAAAAAAAAAAAAA")), graph.target(qid("alpha", "01EDGEGONEAAAAAAAAAAAAAAAA"))]).toEqual([qid("alpha", A), qid("gamma", C)]);
    expect(external).toEqual({ drawn: 1, notOpen: 2, broken: 0 });
  });

  it("makes a neighbour in another open graph navigable by its key, and leaves one in a graph not open inert", async () => {
    const detail = await loadNode(reader().read, "alpha", A);
    const open = new Map([[GA, "alpha"], [GB, "beta"]]);
    const links = {
      target: (n: (typeof detail.neighbours)[number]) => (n.kind === "node" && open.has(n.graph) ? qid(open.get(n.graph)!, n.id) : null),
      graphName: (n: (typeof detail.neighbours)[number]) => open.get(n.graph) ?? n.graphAlias ?? n.graph,
    };
    const picked: string[] = [];
    const html = renderToStaticMarkup(<NeighbourList rows={detail.neighbours} theme="light" onPick={(k) => picked.push(k)} hiddenIds={() => false} links={links} />);
    expect(html.match(/data-testid="neighbour-local"/g)).toHaveLength(1);
    expect(html.match(/data-testid="neighbour-open"/g)).toHaveLength(1);
    expect(html).toContain('data-testid="neighbour-graph"');
    expect(html).not.toContain('data-testid="neighbour-external"');
    // With beta closed, beta's neighbour is labelled and inert, as in a one-graph view.
    const closed = renderToStaticMarkup(<NeighbourList rows={detail.neighbours} theme="light" onPick={() => {}} hiddenIds={() => false} links={{ ...links, target: (n) => (n.kind === "node" && n.graph === GA ? qid("alpha", n.id) : null) }} />);
    expect(closed).toContain('data-testid="neighbour-external"');
    expect(closed).not.toContain('data-testid="neighbour-open"');
  });
});

describe("neighbours outside the graph or unresolved (R170-1, R170-2)", () => {
  it("types each row as a local node, an external node or a state, and keeps its graph", async () => {
    const detail = await loadNode(reader().read, "alpha", A);
    const summary = detail.neighbours.map((n) => [n.kind, n.local, n.graph, n.graphAlias, n.kind === "state" ? n.state : n.title]);
    expect(summary).toEqual([
      ["node", true, GA, "alpha", "alpha b"],
      ["node", false, GB, "beta", "beta b"],
      ["state", false, GC, null, "graph_not_loaded"],
      ["state", false, GB, "beta", "copy_ambiguous"],
      ["state", false, GA, "alpha", "edge_broken"],
    ]);
  });

  it("renders state rows by state and id, labels external nodes, and offers to move only to the local one", async () => {
    const detail = await loadNode(reader().read, "alpha", A);
    const html = renderToStaticMarkup(
      <NeighbourList rows={detail.neighbours} theme="light" onPick={() => {}} hiddenIds={() => false} />,
    );
    expect(html.match(/<button/g)).toHaveLength(1);
    expect(html.match(/data-testid="neighbour-local"/g)).toHaveLength(1);
    expect(html.match(/data-testid="neighbour-external"/g)).toHaveLength(1);
    expect(html.match(/data-testid="neighbour-state"/g)).toHaveLength(3);
    expect(html).toContain("other graph beta");
    expect(html).toContain("graph_not_loaded");
    expect(html).toContain(`graph ${GC}`);
    expect(html).toContain("copy_ambiguous");
    expect(html).toContain("2 candidates");
    expect(html).toContain(C);
  });
});

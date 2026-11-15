import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { NodePanel } from "@/components/node-panel";
import { applyVisibility, buildGraph } from "@/lib/graph/build";
import { qid } from "@/lib/graph/keys";
import { type Focus, focusDepths, focusOf } from "@/lib/graph/focus";
import type { EdgeRow, NodeRow } from "@/lib/types";
import fixture from "./fixtures/walk.json";

// walk.json is recorded from real `umx walk <A> --depth n --direction both` by
// plan 102's walk-fixture.sh (not published): walk_all on the graph, walk_visible on the same
// graph without its retired node R.
type Walks = Record<string, [string, number][]>;

function graph() {
  const { graph } = buildGraph({
    graph: "walkfix",
    graphId: "g",
    nodes: fixture.nodes as NodeRow[],
    edges: fixture.edges as EdgeRow[],
    totalNodes: fixture.nodes.length,
    totalEdges: fixture.edges.length,
    truncated: false,
    omittedEdges: { otherGraph: 0, unresolved: 0 },
  });
  return graph;
}

function focus(retiredShown: boolean, depth: number): [string, number][] {
  const g = graph();
  applyVisibility(g, { provenance: false, retired: retiredShown });
  const start = g.findNode((_id, a) => a.label === "A")!;
  return [...focusDepths(g, start, depth)]
    .filter(([id]) => id !== start)
    .map(([id, d]): [string, number] => [g.getNodeAttribute(id, "label"), d])
    .sort((a, b) => a[0].localeCompare(b[0]));
}

describe("focus matches umx walk --direction both", () => {
  for (const depth of [1, 2, 3]) {
    it(`at ${depth} hops with every node shown`, () => {
      expect(focus(true, depth)).toEqual((fixture.walk_all as unknown as Walks)[depth]);
    });
    it(`at ${depth} hops over the visible graph (retired R hidden)`, () => {
      expect(focus(false, depth)).toEqual((fixture.walk_visible as unknown as Walks)[depth]);
    });
  }

  it("leaves similar_to out, as walk does without --relation similar_to", () => {
    const g = graph();
    const [a, b] = ["A", "G"].map((t) => g.findNode((_id, x) => x.label === t)!);
    g.addEdgeWithKey("sim", a, b, { kind: "similar_to", color: "#000000", size: 1, hidden: false });
    expect(focusDepths(g, a, 1).has(b)).toBe(false);
  });

  it("is empty for a hidden start", () => {
    const g = graph();
    applyVisibility(g, { provenance: false, retired: false });
    const r = g.findNode((_id, a) => a.label === "R")!;
    expect(focusDepths(g, r, 2).size).toBe(0);
  });
});

// Review R197-1: the app's focus state when its node is hidden, with an isolated visible node as the zero-neighbour
// control. A -> B, R (retired) -> A, X alone.
describe("focus state in the app", () => {
  const k = (id: string) => qid("small", id);
  function small() {
    const { graph } = buildGraph({
      graph: "small",
      graphId: "g",
      nodes: [
        ["A", "problem", "Node A", "open"],
        ["B", "problem", "Node B", "open"],
        ["R", "problem", "Node R", "retired"],
        ["X", "problem", "Node X", "open"],
      ],
      edges: [
        ["e1", "supports", "A", "B"],
        ["e2", "supports", "R", "A"],
      ],
      totalNodes: 4,
      totalEdges: 2,
      truncated: false,
      omittedEdges: { otherGraph: 0, unresolved: 0 },
    });
    return graph;
  }

  function panel(id: string, focus: Focus) {
    return renderToStaticMarkup(
      <NodePanel
        open
        detail={{ node: { id, type: "problem", title: `Node ${id}`, status: "retired" }, neighbours: [], neighboursTruncated: false }}
        error={null}
        loading={false}
        theme="light"
        onClose={() => {}}
        onPick={() => {}}
        hiddenIds={() => true}
        focus={{ depth: 1, focus, onDepth: () => {} }}
      />,
    );
  }

  it("shows an isolated visible node alone, with a count of 0", () => {
    const g = small();
    applyVisibility(g, { provenance: false, retired: false });
    const focus = focusOf(g, k("X"), 3);
    expect(focus.state).toBe("on");
    if (focus.state !== "on") return;
    expect(focus.count).toBe(0);
    expect([...focus.depths]).toEqual([[k("X"), 0]]);
    expect(panel("X", focus)).toContain("0 nodes");
  });

  it("pauses, with no count and nothing dimmed, when the focused node is hidden, and resumes when shown", () => {
    const g = small();
    applyVisibility(g, { provenance: false, retired: true });
    const before = focusOf(g, k("R"), 1);
    expect(before.state === "on" && before.count).toBe(1);

    applyVisibility(g, { provenance: false, retired: false });
    const hidden = focusOf(g, k("R"), 1);
    expect(hidden).toEqual({ state: "unavailable" });
    const html = panel("R", hidden);
    expect(html).toContain("Node R");
    expect(html).toContain("Paused: this node is hidden");
    expect(html).not.toMatch(/-\d+ nodes?/);
    expect(html).not.toContain('data-testid="focus-count"');

    applyVisibility(g, { provenance: false, retired: true });
    expect(focusOf(g, k("R"), 1).state).toBe("on");
  });

  it("pauses when an already hidden node is selected with focus on", () => {
    const g = small();
    applyVisibility(g, { provenance: false, retired: false });
    expect(focusOf(g, k("A"), 1).state).toBe("on");
    expect(focusOf(g, k("R"), 1)).toEqual({ state: "unavailable" });
    expect(focusOf(g, k("gone"), 1)).toEqual({ state: "unavailable" });
  });

  it("is off with no selection or depth 0", () => {
    const g = small();
    expect(focusOf(g, null, 2).state).toBe("off");
    expect(focusOf(g, k("A"), 0).state).toBe("off");
  });
});

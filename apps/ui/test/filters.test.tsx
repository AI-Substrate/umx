// Filter chips (req-111) on the synthetic graph searchfix (test/fixtures/search.json): a chip turned off hides
// exactly its nodes or edges, its count is what it hides, and the clusters, focus and labels follow the same rule.
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HitList } from "@/components/hit-list";
import { applyVisibility, buildGraph, chipCounts, chipList, edgeMask, edgeShown, indexGraph, shownMask, type ViewGraph } from "@/lib/graph/build";
import { parentGroups, placePills, type Box } from "@/lib/graph/clusters";
import { type Filters, hiddenReasons, NO_FILTERS, reveal, toggleChip, type Visibility } from "@/lib/graph/filters";
import { emphasis, focusDepths } from "@/lib/graph/focus";
import type { EdgeRow, NodeRow } from "@/lib/types";
import fixture from "./fixtures/search.json";

const HIDE: Visibility = { provenance: false, retired: false };
const SHOW: Visibility = { provenance: true, retired: true };

function graph(): ViewGraph {
  return buildGraph({
    graph: "searchfix",
    graphId: "g",
    nodes: fixture.nodes as NodeRow[],
    edges: fixture.edges as EdgeRow[],
    totalNodes: fixture.nodes.length,
    totalEdges: fixture.edges.length,
    truncated: false,
    omittedEdges: { otherGraph: 0, unresolved: 0 },
  }).graph;
}

function drawn(g: ViewGraph, visible: Visibility, f: Filters) {
  applyVisibility(g, visible, f);
  const nodes = new Set(g.filterNodes((_n, a) => !a.hidden));
  const edges = new Set(g.filterEdges((e) => edgeShown(g, e, f)));
  return { nodes, edges };
}

const byId = (g: ViewGraph, title: string) => g.findNode((_n, a) => a.label === title)!;

describe("a chip off hides exactly its nodes or edges, and its count says how many", () => {
  for (const visible of [HIDE, SHOW]) {
    const label = visible === HIDE ? "provenance and retired hidden" : "everything shown";
    it(`types, ${label}`, () => {
      const g = graph();
      const before = drawn(g, visible, NO_FILTERS);
      const counts = chipCounts(g, visible, NO_FILTERS);
      expect([...counts.types.values()].reduce((a, b) => a + b, 0)).toBe(before.nodes.size);
      for (const [type, count] of counts.types) {
        const after = drawn(g, visible, toggleChip(NO_FILTERS, "types", type));
        const gone = [...before.nodes].filter((n) => !after.nodes.has(n));
        expect(gone.length).toBe(count);
        expect(gone.every((n) => g.getNodeAttribute(n, "kind") === type)).toBe(true);
        expect([...after.nodes].some((n) => g.getNodeAttribute(n, "kind") === type)).toBe(false);
      }
    });
    it(`statuses, ${label}`, () => {
      const g = graph();
      const before = drawn(g, visible, NO_FILTERS);
      const counts = chipCounts(g, visible, NO_FILTERS);
      for (const [status, count] of counts.statuses) {
        const after = drawn(g, visible, toggleChip(NO_FILTERS, "statuses", status));
        const gone = [...before.nodes].filter((n) => !after.nodes.has(n));
        expect(gone.length).toBe(count);
        expect(gone.every((n) => g.getNodeAttribute(n, "status") === status)).toBe(true);
      }
    });
    it(`relations, ${label}: nodes stay, only that relation's edges go`, () => {
      const g = graph();
      const before = drawn(g, visible, NO_FILTERS);
      const counts = chipCounts(g, visible, NO_FILTERS);
      expect([...counts.relations.values()].reduce((a, b) => a + b, 0)).toBe(before.edges.size);
      for (const [relation, count] of counts.relations) {
        const after = drawn(g, visible, toggleChip(NO_FILTERS, "relations", relation));
        expect(after.nodes).toEqual(before.nodes);
        const gone = [...before.edges].filter((e) => !after.edges.has(e));
        expect(gone.length).toBe(count);
        expect(gone.every((e) => g.getEdgeAttribute(e, "kind") === relation)).toBe(true);
      }
    });
  }

  it("the counts of the chips that are on add up to what is drawn, whatever is off", () => {
    const g = graph();
    let f: Filters = NO_FILTERS;
    for (const [dim, key] of [["types", "risk"], ["statuses", "open"], ["relations", "part_of"], ["types", "system"]] as const) {
      f = toggleChip(f, dim, key);
      const { nodes, edges } = drawn(g, HIDE, f);
      const c = chipCounts(g, HIDE, f);
      const on = (m: Map<string, number>, off: ReadonlySet<string>) => [...m].filter(([k]) => !off.has(k)).reduce((a, [, n]) => a + n, 0);
      expect(on(c.types, f.types)).toBe(nodes.size);
      expect(on(c.statuses, f.statuses)).toBe(nodes.size);
      expect(on(c.relations, f.relations)).toBe(edges.size);
      // An off chip stays listed so it can be turned back on.
      expect(chipList(c[dim], f[dim]).map(([k]) => k)).toContain(key);
    }
  });

  it("drops provenance types from the type chips while provenance is hidden (N5)", () => {
    const g = graph();
    expect(chipCounts(g, HIDE, NO_FILTERS).types.has("session")).toBe(false);
    expect(chipCounts(g, SHOW, NO_FILTERS).types.get("session")).toBe(2);
  });
});

describe("clusters and focus follow the filters", () => {
  it("the worker's masks match the canvas", () => {
    const g = graph();
    const f = toggleChip(toggleChip(NO_FILTERS, "types", "risk"), "relations", "blocks");
    applyVisibility(g, HIDE, f);
    const mask = shownMask(g, HIDE, f);
    g.forEachNode((_n, a) => expect(mask[a.index] === 1).toBe(!a.hidden));
    const edges = edgeMask(g, f);
    let e = 0;
    g.forEachEdge((_k, a) => expect(edges[e++] === 1).toBe(a.kind !== "blocks"));
  });

  it("Parent grouping drops part_of when part_of is filtered out", () => {
    const g = graph();
    const index = indexGraph(g);
    const mask = shownMask(g, HIDE);
    const withParts = parentGroups(index, mask, edgeMask(g, NO_FILTERS));
    const without = parentGroups(index, mask, edgeMask(g, toggleChip(NO_FILTERS, "relations", "part_of")));
    expect(Math.max(...withParts.sizes)).toBeGreaterThan(1);
    expect(Math.max(...without.sizes)).toBe(1);
  });

  it("focus does not walk a filtered relation", () => {
    const g = graph();
    applyVisibility(g, HIDE);
    const pump = byId(g, "Water pump for the rain tank");
    const garden = byId(g, "Community garden irrigation");
    expect(focusDepths(g, pump, 1).get(garden)).toBe(1);
    expect(focusDepths(g, pump, 1, new Set(["part_of"])).has(garden)).toBe(false);
  });
});

describe("a hidden result says why and can be shown", () => {
  it("names every reason and reveal undoes only those", () => {
    const g = graph();
    const old = g.getNodeAttributes(byId(g, "Old cracked water tank"));
    const f = toggleChip(toggleChip(NO_FILTERS, "types", "system"), "relations", "blocks");
    expect(hiddenReasons(old, HIDE, f)).toEqual(["retired", "type"]);
    const next = reveal(old, HIDE, f);
    expect(next.visible).toEqual({ provenance: false, retired: true });
    expect([...next.filters.types]).toEqual([]);
    expect([...next.filters.relations]).toEqual(["blocks"]);
    expect(hiddenReasons(old, next.visible, next.filters)).toEqual([]);
    const rec = g.getNodeAttributes(byId(g, "Recording of the bake day"));
    expect(hiddenReasons(rec, HIDE, toggleChip(NO_FILTERS, "statuses", "open"))).toEqual(["provenance", "status"]);
  });

  it("renders the reasons, a Show button and umx's warnings as given", () => {
    const html = renderToStaticMarkup(
      <HitList
        result={{
          mode: "hybrid",
          query: "tank",
          hits: [
            { id: "01ZZZZZZZZZZZZZZZZZZZZZZZA", type: "system", title: "Shown tank", status: "open", score: 0.03, legs: "both", exact: null },
            { id: "01ZZZZZZZZZZZZZZZZZZZZZZZB", type: "system", title: "Old tank", status: "retired", score: 0.02, legs: "word", exact: null },
          ],
          warnings: [{ code: "similarity_unavailable", message: "Similarity was not used for demo: it has no vectors yet.", next: null }],
          note: null,
          foreign: 0,
        }}
        theme="light"
        place={(id) => (id.endsWith("A") ? { kind: "shown" } : { kind: "hidden", reasons: ["retired", "type system filtered"] })}
        onPick={() => {}}
        onReveal={() => {}}
        testId="search-results"
      />,
    );
    expect(html).toContain("similarity_unavailable</span>: Similarity was not used for demo: it has no vectors yet.");
    expect(html).toContain("Hidden: retired, type system filtered");
    expect(html.match(/data-testid="hit-show"/g)?.length).toBe(1);
  });
});

describe("ui-expert notes", () => {
  it("N9: focus forces labels for the selected node and hop 1 only", () => {
    const focused = new Map([["s", 0], ["a", 1], ["b", 2], ["c", 3]]);
    const near = new Set(["a"]);
    expect(["s", "a", "b", "c", "x"].map((n) => emphasis(n, "s", near, focused))).toEqual(["selected", "label", "near", "near", "dim"]);
    expect(emphasis("a", "s", near, null)).toBe("label");
    expect(emphasis("z", "s", near, null)).toBe("plain");
  });

  it("N10: a cluster pill overlapping one already drawn is skipped, biggest group first", () => {
    const boxes: Box[] = [
      [0, 0, 100, 22], // biggest group
      [90, 10, 190, 32], // overlaps the first: skipped
      [200, 0, 300, 22], // clear
      [100, 0, 200, 22], // touches the first edge to edge only: drawn
      [250, 21, 350, 43], // overlaps the third by 1 px: skipped
    ];
    expect(placePills(boxes)).toEqual([true, false, true, true, false]);
  });
});

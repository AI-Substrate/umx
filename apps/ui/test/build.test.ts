import { describe, expect, it } from "vitest";
import { applyVisibility, buildGraph, counts } from "@/lib/graph/build";

describe("legend counts (ui-expert N5)", () => {
  it("lists a provenance type only while provenance is shown, and counts only what is on the canvas", () => {
    const { graph } = buildGraph({
      graph: "g",
      graphId: "g",
      nodes: [
        ["N1", "problem", "a", "open"],
        ["N2", "session", "b", "open"],
        ["N3", "problem", "c", "retired"],
      ],
      edges: [],
      totalNodes: 3,
      totalEdges: 0,
      truncated: false,
      omittedEdges: { otherGraph: 0, unresolved: 0 },
    });
    applyVisibility(graph, { provenance: false, retired: false });
    const hidden = counts(graph);
    expect([...hidden.types]).toEqual([["problem", 1]]);
    expect(hidden.provenance).toBe(1);
    expect(hidden.retired).toBe(1);
    applyVisibility(graph, { provenance: true, retired: true });
    expect(new Map(counts(graph).types)).toEqual(new Map([["problem", 2], ["session", 1]]));
  });
});

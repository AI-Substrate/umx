// Node shapes by type (lib/graph/shapes.ts, plan 198): the ui-expert's table for every std and starter type, the
// fallback, and geometry with the disc's visual weight (review B1) that the program's triangle still holds.
import { describe, expect, it } from "vitest";
import { applyRows } from "@/lib/graph/apply";
import { buildGraph } from "@/lib/graph/build";
import { DEFAULT_SHAPE, extentOf, GEOMETRY, type Shape, SHAPE_REACH, shapeAt, shapeIndex, shapeOf, shapePath, SHAPES, svgPath } from "@/lib/graph/shapes";
import type { Snapshot } from "@/lib/types";

// The note's table (the UI expert's design note, section 1), written out here, not read from the code: std v1's
// types (umx-core standard.rs USER_TYPES and the provenance types) and the starter vocabulary (ontology.rs).
const TABLE: Record<string, Shape> = {
  topic: "roundRect", project: "roundRect", action: "roundRect", capability: "roundRect", criterion: "roundRect", claim: "roundRect",
  system: "roundRect", place: "roundRect", document: "roundRect", transcript: "roundRect", file: "roundRect", url: "roundRect",
  folder: "roundRect", excerpt: "roundRect",
  person: "circle", session: "circle", agent: "circle", harness: "circle", model: "circle", platform: "circle",
  team: "ellipse", organisation: "ellipse", role: "ellipse",
  option: "hexagon",
  question: "diamond",
  decision: "tag",
  problem: "cutRect", risk: "cutRect", tension: "cutRect",
  meeting: "pill", event: "pill", segment: "pill",
  // Starter types the table does not name: the default.
  actor: "roundRect", mechanism: "roundRect", precedent: "roundRect",
};

describe("shapes by type (plan 198)", () => {
  it("maps every std and starter type to its row of the table", () => {
    for (const [type, shape] of Object.entries(TABLE)) expect([type, shapeOf(type)]).toEqual([type, shape]);
  });

  it("draws an unknown type as the rounded rectangle, including names an object already has", () => {
    expect(DEFAULT_SHAPE).toBe("roundRect");
    for (const type of ["widget", "", "constructor", "toString", "__proto__", "Person"]) expect(shapeOf(type)).toBe("roundRect");
    expect(shapeAt(undefined)).toBe("roundRect");
    expect(shapeAt(99)).toBe("roundRect");
    expect(shapeAt("3")).toBe("roundRect");
    expect(shapeAt(shapeIndex("option"))).toBe("hexagon");
  });

  it("sets each node's shape number from its type, and again when a live change gives it another type", () => {
    const snap: Snapshot = {
      graph: "demo", graphId: "g-demo", nodes: [["01AAAAAAAAAAAAAAAAAAAAAAAA", "question", "Why", "open"]], edges: [],
      totalNodes: 1, totalEdges: 0, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
    };
    const { graph } = buildGraph(snap);
    const key = graph.nodes()[0];
    expect(SHAPES[graph.getNodeAttribute(key, "shape")]).toBe("diamond");
    applyRows(graph, { nodes: [[key, "decision", "Why", "open"]], edges: [] }, { nodes: [], edges: [] });
    expect(SHAPES[graph.getNodeAttribute(key, "shape")]).toBe("tag");
  });

  it("gives every shape the disc's weight: the full diameter across, at least 0.7 of it tall, area within 15%", () => {
    // The outline as the 2D layers trace it (shapePath), sampled into a polygon: measured here, not by extentOf.
    const outline = (shape: Shape) => {
      const pts: [number, number][] = [];
      const arc = (x: number, y: number, rx: number, ry: number, a0: number, a1: number) => {
        for (let k = 0; k <= 64; k++) pts.push([x + rx * Math.cos(a0 + ((a1 - a0) * k) / 64), y + ry * Math.sin(a0 + ((a1 - a0) * k) / 64)]);
      };
      shapePath(
        { moveTo: (x, y) => pts.push([x, y]), lineTo: (x, y) => pts.push([x, y]), arc: (x, y, r, a0, a1) => arc(x, y, r, r, a0, a1), ellipse: (x, y, rx, ry, _r, a0, a1) => arc(x, y, rx, ry, a0, a1), closePath: () => {} },
        shape, 0, 0, 1,
      );
      const area = Math.abs(pts.reduce((s, [x, y], i) => s + x * pts[(i + 1) % pts.length][1] - pts[(i + 1) % pts.length][0] * y, 0)) / 2;
      return { area, halfWidth: Math.max(...pts.map(([x]) => Math.abs(x))), halfHeight: Math.max(...pts.map(([, y]) => Math.abs(y))), reach: Math.max(...pts.map(([x, y]) => Math.hypot(x, y))) };
    };
    for (const shape of SHAPES) {
      const m = outline(shape);
      expect(m.halfWidth, shape).toBeGreaterThanOrEqual(1 - 1e-6);
      expect(m.halfHeight, shape).toBeGreaterThanOrEqual(0.7);
      expect(Math.abs(m.area - Math.PI) / Math.PI, shape).toBeLessThanOrEqual(0.15);
      // The program's triangle holds the farthest point, and the label sits past the widest one.
      expect(m.reach, shape).toBeLessThanOrEqual(SHAPE_REACH + 1e-6);
      expect(extentOf(shape).halfWidth, shape).toBeCloseTo(m.halfWidth, 2);
      const g = GEOMETRY[shape];
      if (g.kind === "poly" && g.points.length > 2) {
        const angles = g.points.map(([x, y]) => Math.atan2(y, x));
        const turns = angles.map((a, i) => (angles[(i + 1) % angles.length] - a + Math.PI * 2) % (Math.PI * 2));
        expect(turns.every((t) => t > 0 && t < Math.PI), shape).toBe(true);
      }
    }
  });

  it("starts every shape as a new subpath at its outline, so a path drawn before it is never joined (reviewer R370-1)", () => {
    for (const shape of SHAPES) {
      // A canvas joins an arc to the subpath before it with a line unless a moveTo comes first: record what is traced.
      const ops: { op: string; args: number[] }[] = [];
      const rec = (op: string) => (...args: number[]) => void ops.push({ op, args });
      shapePath({ moveTo: rec("moveTo"), lineTo: rec("lineTo"), arc: rec("arc"), ellipse: rec("ellipse"), closePath: rec("closePath") }, shape, 10, 20, 5);
      expect(ops[0].op, shape).toBe("moveTo");
      const next = ops[1];
      if (next.op === "arc") {
        const [cx, cy, r, a0] = next.args;
        expect(ops[0].args[0], shape).toBeCloseTo(cx + r * Math.cos(a0), 9);
        expect(ops[0].args[1], shape).toBeCloseTo(cy + r * Math.sin(a0), 9);
      }
      expect(ops.at(-1)!.op, shape).toBe("closePath");
    }
  });

  it("gives every shape a closed swatch path inside its box", () => {
    for (const shape of SHAPES) {
      const d = svgPath(shape, 10);
      expect(d.endsWith("Z"), shape).toBe(true);
      const numbers = d.match(/-?\d+(\.\d+)?/g)!.map(Number);
      expect(numbers.every((v) => v >= 0 && v <= 10), `${shape}: ${d}`).toBe(true);
    }
  });
});

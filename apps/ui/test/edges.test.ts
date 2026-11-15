// Edges on the canvas (lib/graph/edges.ts, plan 150): which program draws an edge, the quiet relations, the chip
// cycle, the 40 forced labels and their order, the chevron's side, and curves after a live change.
import { describe, expect, it, vi } from "vitest";
import { agentFilter, keepPersonRelations, LEDGER0, personSets } from "@/lib/agent";
import { applyRows, qualifyRows, readChange, snapAfter } from "@/lib/graph/apply";
import { applyVisibility, buildGraph, edgeShown, type ViewGraph } from "@/lib/graph/build";
import {
  curvatures,
  cycleRelation,
  EDGE_SIZE,
  edgeDrawn,
  edgeLabelBox,
  edgeProgram,
  headProgram,
  HEAD,
  HUB_HEAD,
  HUB_IN,
  incomingDrawn,
  selectedEdgeColor,
  forcedLabels,
  forcedShown,
  fitsZoomRule,
  FORCED_LABELS_MAX,
  HOVER_LABELS_MAX,
  labelFrame,
  LABELS_PER_FRAME,
  SMALL_GRAPH_EDGES,
  isSymmetric,
  labelText,
  nodeLabelBox,
  overlaps,
  paintEdgeLabel,
  QUIET_RELATIONS,
  relationMode,
  relationSymmetric,
  type RelationModes,
  symmetricByGraph,
} from "@/lib/graph/edges";
import { edgeLeftOut, type Entities, type LeftOut, NO_ENTITIES } from "@/lib/graph/entities";
import { NO_FILTERS, toggleChip } from "@/lib/graph/filters";
import { aliasOfKey, qid } from "@/lib/graph/keys";
import { NOTHING, toRead } from "@/lib/live";
import type { RowsReply, Wanted } from "@/lib/rows";
import type { EdgeRow, NodeRow, Snapshot } from "@/lib/types";

const id = (c: string, n = 0) => `01${c.repeat(20)}${String(n).padStart(4, "0")}`;
const edgeId = (n: number) => `01EDGE${String(n).padStart(20, "0")}`;

function graphOf(nodes: string[], edges: [string, string, string, string][], graph = "demo"): ViewGraph {
  const rows: NodeRow[] = nodes.map((n) => [n, "problem", n, "open"]);
  const e: EdgeRow[] = edges.map(([eid, kind, s, t]) => [eid, kind, s, t]);
  return buildGraph({ graph, graphId: `g-${graph}`, nodes: rows, edges: e, totalNodes: rows.length, totalEdges: e.length, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 } }).graph;
}

describe("arrows point source to target; symmetric relations are plain lines", () => {
  const symmetric = new Map([["alpha", new Set(["rhymes_with"])]]);
  it("picks arrow or line per relation, a graph's own symmetric relations only in that graph", () => {
    const program = (edge: string, relation: string) => edgeProgram(isSymmetric(edge, relation, symmetric), 0);
    expect(program(qid("alpha", "e1"), "supports")).toBe("arrow");
    expect(program(qid("alpha", "e2"), "conflicts_with")).toBe("line");
    expect(program(qid("alpha", "e3"), "similar_to")).toBe("line");
    expect(program(qid("alpha", "e4"), "rhymes_with")).toBe("line");
    expect(program(qid("beta", "e5"), "rhymes_with")).toBe("arrow");
    expect(edgeProgram(false, 0.25)).toBe("curvedArrow");
    expect(edgeProgram(true, -0.25)).toBe("curve");
    expect(relationSymmetric("rhymes_with", symmetric)).toBe(true);
    expect(relationSymmetric("supports", symmetric)).toBe(false);
  });
});

describe("quiet context relations and the chip cycle", () => {
  it("starts the UI's list quiet and everything else, unknown relations too, normal", () => {
    expect([...QUIET_RELATIONS].sort()).toEqual(["attended", "held_at", "mentions", "part_of", "raised_by", "raised_in", "sourced_from"]);
    const none: RelationModes = new Map();
    for (const r of QUIET_RELATIONS) expect(relationMode(r, none, new Set())).toBe("quiet");
    expect(relationMode("supports", none, new Set())).toBe("normal");
    expect(relationMode("never_seen_before", none, new Set())).toBe("normal");
    expect(relationMode("raised_in", none, new Set(["raised_in"]))).toBe("off");
  });

  it("cycles quiet, normal, off and back, keeping only choices away from the default", () => {
    const walk = (relation: string) => {
      let modes: RelationModes = new Map();
      let off = new Set<string>();
      const seen = [relationMode(relation, modes, off)];
      for (let i = 0; i < 3; i++) {
        const next = cycleRelation(relation, modes, off);
        modes = next.modes;
        off = next.off ? new Set([relation]) : new Set();
        seen.push(relationMode(relation, modes, off));
      }
      return { seen, modes };
    };
    const quiet = walk("raised_in");
    expect(quiet.seen).toEqual(["quiet", "normal", "off", "quiet"]);
    expect(quiet.modes.size).toBe(0);
    const normal = walk("supports");
    expect(normal.seen).toEqual(["normal", "off", "quiet", "normal"]);
    expect(normal.modes.size).toBe(0);
  });
});

describe("forced labels: the selected node's and focus hop 1's up to 150, the hovered node's up to 40 more", () => {
  const hub = id("H");
  const other = id("O");
  const centre = id("C");
  const leaves = Array.from({ length: 230 }, (_v, i) => id("L", i));
  const edges: [string, string, string, string][] = [
    ...leaves.slice(0, 160).map((l, i): [string, string, string, string] => [edgeId(i), "supports", hub, l]),
    ...leaves.slice(160, 210).map((l, i): [string, string, string, string] => [edgeId(1000 + i), "raised_in", l, other]),
    ...leaves.slice(210).map((l, i): [string, string, string, string] => [edgeId(2000 + i), "supports", centre, l]),
  ];
  const g = graphOf([hub, other, centre, ...leaves], edges);
  const k = (n: string) => qid("demo", n);
  const all = () => true;
  const touching = (list: string[], n: string) => list.filter((e) => g.extremities(e).includes(k(n))).length;

  it("takes 150 of a selected hub's edges and counts the rest, then 40 of the hovered node's, and no focus edge past 150", () => {
    expect([FORCED_LABELS_MAX, HOVER_LABELS_MAX]).toEqual([150, 40]);
    const out = forcedLabels(g, { selected: k(hub), hovered: k(other), focusCentre: k(centre) }, all);
    const list = [...out.edges];
    expect(list).toHaveLength(190);
    expect(list.slice(0, 150).every((e) => g.extremities(e).includes(k(hub)))).toBe(true);
    expect(list.slice(150).every((e) => g.extremities(e).includes(k(other)))).toBe(true);
    expect(touching(list, centre)).toBe(0);
    expect(out.more).toBe(10);
  });

  it("fills the 150 with the focus centre's edges after the selected node's; the hover's 40 never take their room", () => {
    const out = forcedLabels(g, { selected: k(hub), hovered: k(other), focusCentre: k(centre) }, all, 165);
    const list = [...out.edges];
    expect([touching(list, hub), touching(list, other), touching(list, centre)]).toEqual([160, 40, 5]);
    expect(out.more).toBe(0);
    const hovered = forcedLabels(g, { selected: null, hovered: k(other), focusCentre: k(centre) }, all);
    expect([touching([...hovered.edges], other), touching([...hovered.edges], centre)]).toEqual([40, 20]);
  });

  it("counts the selected node's edges past the cap, and leaves out edges not drawn", () => {
    const sel = forcedLabels(g, { selected: k(hub), hovered: null, focusCentre: k(centre) }, all, 25);
    expect(sel.edges.size).toBe(25);
    expect(sel.more).toBe(135);
    g.setNodeAttribute(k(leaves[0]), "hidden", true);
    const drawn = forcedLabels(g, { selected: k(hub), hovered: null, focusCentre: null }, edgeDrawn(g, new Set(), null));
    expect([drawn.edges.size, drawn.more]).toEqual([150, 9]);
    expect(forcedLabels(g, { selected: k(hub), hovered: null, focusCentre: null }, edgeDrawn(g, new Set(["supports"]), null)).edges.size).toBe(0);
    g.setNodeAttribute(k(leaves[0]), "hidden", false);
  });
});

describe("one frame's edge labels (plan 198): forced first, every label on a small graph, the zoom rule, 300 at most", () => {
  const items = (n: number) => Array.from({ length: n }, (_v, i) => ({ edge: `e${i}` }));
  const run = (forced: string[], drawn: { edge: string }[], other: (item: { edge: string }, every: boolean) => boolean = () => true) => {
    const calls: string[] = [];
    const out = labelFrame(forced, drawn, {
      forced: (edge) => (calls.push(`f:${edge}`), true),
      other: (item, every) => (calls.push(`${every ? "every" : "rule"}:${item.edge}`), other(item, every)),
    });
    return { out, calls };
  };

  it("labels every drawn edge, with no fit test, when 150 or fewer are drawn, and goes by the rule at 151", () => {
    expect(SMALL_GRAPH_EDGES).toBe(150);
    const small = run([], items(150));
    expect(small.calls.every((c) => c.startsWith("every:"))).toBe(true);
    expect(small.out).toEqual({ forced: 0, other: 150 });
    const big = run([], items(151));
    expect(big.calls.every((c) => c.startsWith("rule:"))).toBe(true);
  });

  it("draws the forced labels first, never twice, and stops at 300 labels in all", () => {
    expect(LABELS_PER_FRAME).toBe(300);
    const forced = Array.from({ length: 150 }, (_v, i) => `e${i * 2}`);
    const { out, calls } = run(forced, items(1000));
    expect(out).toEqual({ forced: 150, other: 150 });
    expect(calls.slice(0, 150).every((c) => c.startsWith("f:"))).toBe(true);
    const painted = calls.filter((c) => !c.startsWith("f:")).map((c) => c.slice(5));
    expect(painted.some((e) => forced.includes(e))).toBe(false);
  });

  it("counts only the rule labels that were drawn against the cap", () => {
    const { out, calls } = run([], items(1000), (item) => Number(item.edge.slice(1)) % 2 === 0);
    expect(out.other).toBe(300);
    expect(calls).toHaveLength(599);
  });

  it("the zoom rule: the length on screen holds the label, both radii and 16 px, and is at least 80 px", () => {
    expect(fitsZoomRule(60 + 5 + 7 + 16, 60, 5, 7)).toBe(true);
    expect(fitsZoomRule(60 + 5 + 7 + 15.9, 60, 5, 7)).toBe(false);
    // A short label on a short edge (ui-expert N1): 30 px would hold it, 80 is the floor.
    expect(fitsZoomRule(79.9, 10, 2, 2)).toBe(false);
    expect(fitsZoomRule(80, 10, 2, 2)).toBe(true);
  });
});

describe("the chevron sits on the side the edge points to, the text upright", () => {
  type Call = { op: string; args: unknown[] };
  function ctx() {
    const calls: Call[] = [];
    const rec = (op: string) => (...args: unknown[]) => void calls.push({ op, args });
    return {
      calls,
      c: {
        save: rec("save"), restore: rec("restore"), translate: rec("translate"), rotate: rec("rotate"), strokeText: rec("strokeText"), fillText: rec("fillText"),
        measureText: (t: string) => ({ width: t.length * 6 }) as TextMetrics,
        font: "", fillStyle: "" as unknown, strokeStyle: "" as unknown, lineWidth: 0, lineJoin: "miter" as CanvasLineJoin, textAlign: "start" as CanvasTextAlign, textBaseline: "alphabetic" as CanvasTextBaseline,
      },
    };
  }
  const style = { font: "sans", color: "#111", halo: "#fff" };

  it("reads 'relation ›' left to right and '‹ relation' right to left, with a halo under the fill", () => {
    expect(labelText("supports", { x: 0, y: 0 }, { x: 100, y: 0 })).toBe("supports ›");
    expect(labelText("supports", { x: 100, y: 0 }, { x: 0, y: 0 })).toBe("‹ supports");
    expect(labelText("supports", { x: 0, y: 0 }, { x: 0, y: 100 })).toBe("supports ›");
    expect(labelText("supports", { x: 0, y: 100 }, { x: 0, y: 0 })).toBe("‹ supports");
    for (const [s, t, text] of [
      [{ x: 0, y: 0 }, { x: 200, y: 50 }, "supports ›"],
      [{ x: 200, y: 50 }, { x: 0, y: 0 }, "‹ supports"],
    ] as const) {
      const { calls, c } = ctx();
      expect(paintEdgeLabel(c, "supports", { ...s, size: 5 }, { ...t, size: 5 }, 0, style, false)).toBe(true);
      const angle = calls.find((x) => x.op === "rotate")!.args[0] as number;
      expect(Math.abs(angle)).toBeLessThanOrEqual(Math.PI / 2);
      expect(calls.filter((x) => x.op === "strokeText" || x.op === "fillText").map((x) => [x.op, x.args[0]])).toEqual([["strokeText", text], ["fillText", text]]);
      expect(c.lineWidth).toBe(3);
      expect(c.font).toBe("500 11px sans");
    }
  });

  it("draws a zoom-rule label once the edge holds the label, both radii and 16 px (plan 198); a forced one wherever there is room", () => {
    // "supports ›" measures 60 here; radii 5 and 7: the rule needs 88 px.
    const at = (length: number, fit: boolean) => paintEdgeLabel(ctx().c, "supports", { x: 0, y: 0, size: 5 }, { x: length, y: 0, size: 7 }, 0, style, fit);
    expect(at(88, true)).toBe(true);
    expect(at(87.9, true)).toBe(false);
    // Room past both nodes for the text alone (E1's old test) is not enough.
    expect(at(75, true)).toBe(false);
    expect(at(75, false)).toBe(true);
    expect(at(12, false)).toBe(false);
  });

  it("skips a zoom-rule label whose box meets a drawn node label, never a forced one (ui-expert E1-N3)", () => {
    const s = { x: 0, y: 100, size: 5 };
    const t = { x: 300, y: 100, size: 5 };
    // A node label drawn across the edge's middle (150, 100), and one well clear of it.
    const across = nodeLabelBox(140, 96, 4, 12, 60);
    const clear = nodeLabelBox(140, 160, 4, 12, 60);
    const meets = (labels: ReturnType<typeof nodeLabelBox>[]) => (box: ReturnType<typeof nodeLabelBox>) => labels.some((b) => overlaps(b, box));
    const drew = (avoid: (b: ReturnType<typeof nodeLabelBox>) => boolean, fit: boolean) => {
      const { calls, c } = ctx();
      paintEdgeLabel(c, "supports", s, t, 0, style, fit, avoid);
      return calls.some((x) => x.op === "fillText");
    };
    expect(drew(meets([across]), true)).toBe(false);
    expect(drew(meets([clear]), true)).toBe(true);
    expect(drew(meets([]), true)).toBe(true);
    expect(drew(meets([across]), false)).toBe(true);
    // A turned label's box grows to hold it.
    const flat = edgeLabelBox({ x: 0, y: 0 }, 0, 60);
    const steep = edgeLabelBox({ x: 0, y: 0 }, Math.PI / 2, 60);
    expect([flat.x1 - flat.x0, steep.y1 - steep.y0]).toEqual([63, 63]);
  });
});

describe("curves for parallel and opposite pairs follow live changes", () => {
  const [rA, rB, rC] = ["A", "B", "C"].map((c) => id(c));
  const k = (n: string) => qid("demo", n);

  it("bends an opposite pair to either side and straightens the one left after a live remove", () => {
    const g = graphOf([rA, rB, rC], [[edgeId(1), "supports", rA, rB], [edgeId(2), "raises", rB, rA], [edgeId(3), "supports", rB, rC]]);
    const [e1, e2, e3] = [1, 2, 3].map((n) => k(edgeId(n)));
    const c = curvatures(g);
    expect(c.has(e3)).toBe(false);
    // Each bends from its own direction; drawn the other way round, equal curvature puts them on opposite sides.
    expect(c.get(e1)).toBeCloseTo(-0.25);
    expect(c.get(e2)).toBeCloseTo(-0.25);
    expect(edgeProgram(false, c.get(e1)!)).toBe("curvedArrow");

    applyRows(g, { nodes: [], edges: [] }, { nodes: [], edges: [e2] });
    const after = curvatures(g);
    expect(after.size).toBe(0);
    expect(edgeProgram(false, after.get(e1) ?? 0)).toBe("arrow");

    applyRows(g, { nodes: [], edges: [[k(edgeId(4)), "supports", k(rA), k(rB)], [k(edgeId(5)), "raises", k(rA), k(rB)]] }, { nodes: [], edges: [] });
    const three = curvatures(g);
    expect([e1, k(edgeId(4)), k(edgeId(5))].map((e) => three.get(e) ?? 0)).toEqual([-0.25, 0, 0.25]);
  });
});

describe("a relation declared symmetric while the view is open (review B1)", () => {
  const [rA, rB, rC] = ["A", "B", "C"].map((c) => id(c));
  const snapOf = (graph: string): Snapshot => ({
    graph, graphId: `g-${graph}`, nodes: [rA, rB, rC].map((n): NodeRow => [n, "option", n, "open"]), edges: [[edgeId(1), "rivals", rA, rB]],
    symmetric: ["conflicts_with"], totalNodes: 3, totalEdges: 1, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
  });

  it("redraws alpha's rivals edges as lines after a change that touched no row, and leaves beta's alone", async () => {
    let snaps = [snapOf("alpha"), snapOf("beta")];
    const { graph } = buildGraph(snaps);
    const program = (alias: string, e: string) => edgeProgram(isSymmetric(qid(alias, e), "rivals", symmetricByGraph(snaps)), 0);
    expect([program("alpha", edgeId(1)), program("beta", edgeId(1))]).toEqual(["arrow", "arrow"]);

    // `umx ontology add relation rivals --symmetric --graph alpha`: a revision whose change names no ids.
    const post = vi.fn(async (body: Wanted): Promise<RowsReply> => ({ graph: "alpha", nodes: [], edges: [], symmetric: ["conflicts_with", "rivals"], ...(body.with ? { external: [] } : {}) }));
    const want = toRead(NOTHING);
    const rows = await readChange(post, want, ["beta"]);
    expect(post).toHaveBeenCalledWith({ nodes: [], edges: [], with: ["beta"] });
    const outcome = applyRows(graph, qualifyRows("alpha", rows, new Set(["alpha", "beta"])), { nodes: [], edges: [] });
    snaps = snaps.map((sn) => (sn.graph === "alpha" ? snapAfter(sn, outcome, rows) : sn));
    expect([program("alpha", edgeId(1)), program("beta", edgeId(1))]).toEqual(["line", "arrow"]);

    // A rivals edge added later in alpha is a line from the start; a reply without the list keeps the last one.
    applyRows(graph, { nodes: [], edges: [[qid("alpha", edgeId(2)), "rivals", qid("alpha", rB), qid("alpha", rC)]] }, { nodes: [], edges: [] });
    expect(program("alpha", edgeId(2))).toBe("line");
    expect(snapAfter(snaps[0], outcome, {}).symmetric).toEqual(["conflicts_with", "rivals"]);
  });
});

describe("forced labels across two graphs holding the same ids (review N2)", () => {
  it("labels the selected copy's edges first and never mixes up the other graph's equal ULIDs", () => {
    const hub = id("H");
    const leaves = Array.from({ length: 30 }, (_v, i) => id("L", i));
    const snap = (graph: string): Snapshot => ({
      graph, graphId: `g-${graph}`, nodes: [hub, ...leaves].map((n): NodeRow => [n, "option", n, "open"]),
      edges: leaves.map((l, i): EdgeRow => [edgeId(i), "supports", hub, l]), totalNodes: 31, totalEdges: 30, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
    });
    const { graph } = buildGraph([snap("alpha"), snap("beta")]);
    const out = forcedLabels(graph, { selected: qid("alpha", hub), hovered: qid("beta", hub), focusCentre: null }, () => true);
    const list = [...out.edges];
    expect(list).toHaveLength(60);
    expect(list.slice(0, 30).every((e) => aliasOfKey(e) === "alpha")).toBe(true);
    expect(list.slice(30).every((e) => aliasOfKey(e) === "beta")).toBe(true);
    expect(out.more).toBe(0);
    const capped = forcedLabels(graph, { selected: qid("beta", hub), hovered: null, focusCentre: null }, () => true, 25);
    expect([...capped.edges].every((e) => aliasOfKey(e) === "beta")).toBe(true);
    expect(capped.more).toBe(5);
  });
});

describe("the person's relation chips across a change of open graphs (review B2)", () => {
  it("keeps the person's off, quiet and normal choices and drops the agent's chip changes", () => {
    const g = (alias: string) =>
      graphOf([id("A"), id("B")], [[edgeId(1), "supports", id("A"), id("B")], [edgeId(2), "blocks", id("A"), id("B")], [edgeId(3), "raised_in", id("B"), id("A")], [edgeId(4), "part_of", id("B"), id("A")]], alias);
    // In alpha: the person turns supports off and raised_in normal; an agent hides blocks.
    let ledger = personSets(LEDGER0, toggleChip(LEDGER0.filters, "relations", "supports"));
    ledger = agentFilter(ledger, 7, { show: [], hide: ["relation:blocks"] });
    const modes: RelationModes = cycleRelation("raised_in", new Map(), ledger.filters.relations).modes;
    expect(ledger.filters.relations).toEqual(new Set(["supports", "blocks"]));

    // Switch to beta, then back to alpha: each change of the open set keeps the person's chips only.
    for (const alias of ["beta", "alpha"]) {
      ledger = keepPersonRelations(ledger);
      const view = g(alias);
      const off = ledger.filters.relations;
      expect([..."supports blocks raised_in part_of".split(" ")].map((r) => relationMode(r, modes, off))).toEqual(["off", "normal", "normal", "quiet"]);
      applyVisibility(view, { provenance: false, retired: false }, ledger.filters);
      const shown = view.filterEdges((e) => edgeShown(view, e, ledger.filters)).map((e) => view.getEdgeAttribute(e, "kind"));
      expect(shown.sort()).toEqual(["blocks", "part_of", "raised_in"]);
      expect(ledger.agent).toEqual([]);
    }
    // Reset filters is still the one reset.
    expect(personSets(ledger, NO_FILTERS, "all").filters.relations.size).toBe(0);
  });
});

describe("heads into a hub and a selected hub's colours (ui-expert E1-N1, E1-N2)", () => {
  const hub = (n: number) =>
    graphOf(
      [id("H"), ...Array.from({ length: n }, (_v, i) => id("S", i))],
      Array.from({ length: n }, (_v, i) => [edgeId(i), "supports", id("S", i), id("H")] as [string, string, string, string]),
    );

  it("keeps every head at 20 drawn incoming edges, and caps a wide edge's head at 6 x 4.8 px past 20", () => {
    for (const [n, want] of [[20, "arrow"], [21, "arrowHub"]] as const) {
      const g = hub(n);
      const incoming = incomingDrawn(g, () => false).get(qid("demo", id("H"))) ?? 0;
      expect(incoming).toBe(n);
      expect(headProgram("arrow", EDGE_SIZE.emphasis, incoming)).toBe(want);
      expect(headProgram("curvedArrow", EDGE_SIZE.emphasis, incoming)).toBe(want === "arrow" ? "curvedArrow" : "curvedArrowHub");
      // A normal or quiet edge's head is already at most 6 x 4.8; a line has none.
      expect([headProgram("arrow", EDGE_SIZE.normal, incoming), headProgram("arrow", EDGE_SIZE.quiet, incoming), headProgram("line", EDGE_SIZE.emphasis, incoming)]).toEqual(["arrow", "arrow", "line"]);
    }
    expect(HUB_IN).toBe(20);
    expect([HUB_HEAD.lengthToThicknessRatio * EDGE_SIZE.emphasis, HUB_HEAD.widenessToThicknessRatio * EDGE_SIZE.emphasis].map((v) => +v.toFixed(6))).toEqual([6, 4.8]);
    expect([HEAD.lengthToThicknessRatio * EDGE_SIZE.normal, HEAD.widenessToThicknessRatio * EDGE_SIZE.normal].map((v) => +v.toFixed(6))).toEqual([6, 4.8]);
  });

  const rules = (over: Partial<LeftOut> = {}): LeftOut => ({ off: new Set(), together: () => false, entities: NO_ENTITIES, proposedPairs: new Set(), ...over });

  it("counts only drawn edges: a relation turned off, or a hidden end, does not make a hub", () => {
    const g = hub(21);
    expect(incomingDrawn(g, edgeLeftOut(g, rules({ off: new Set(["supports"]) }))).get(qid("demo", id("H")))).toBeUndefined();
    g.setNodeAttribute(qid("demo", id("S", 0)), "hidden", true);
    expect(incomingDrawn(g, edgeLeftOut(g, rules())).get(qid("demo", id("H")))).toBe(20);
  });

  it("does not count an edge the reducer leaves out: 20 drawn plus a denial, a threaded same_as or a merged inner edge stays normal (reviewer B1)", () => {
    const H = qid("demo", id("H"));
    const extra = (kind: string) =>
      graphOf(
        [id("H"), id("D"), ...Array.from({ length: 20 }, (_v, i) => id("S", i))],
        [...Array.from({ length: 20 }, (_v, i) => [edgeId(i), "supports", id("S", i), id("H")] as [string, string, string, string]), [edgeId(99), kind, id("D"), id("H")]],
      );
    const D = qid("demo", id("D"));
    const thread: Entities = { list: [], of: new Map([[D, 0], [H, 0]]), proposed: [] };
    for (const [g, r] of [
      [extra("not_same_as"), rules()],
      [extra("same_as"), rules({ entities: thread })],
      [extra("possibly_same_as"), rules({ proposedPairs: new Set([`${D}|${H}`, `${H}|${D}`]) })],
      [extra("supports"), rules({ together: (s, t) => s === D && t === H })],
    ] as const) {
      const incoming = incomingDrawn(g, edgeLeftOut(g, r)).get(H) ?? 0;
      expect(incoming).toBe(20);
      expect(headProgram("arrow", EDGE_SIZE.emphasis, incoming)).toBe("arrow");
    }
    // The same 21st edge, drawn, makes a hub.
    const drawn = incomingDrawn(extra("supports"), edgeLeftOut(extra("supports"), rules())).get(H) ?? 0;
    expect([drawn, headProgram("arrow", EDGE_SIZE.emphasis, drawn)]).toEqual([21, "arrowHub"]);
  });

  it("never forces a label on, or counts in +n, an edge the reducer leaves out: a selected denial or a threaded identity edge (reviewer R370-2)", () => {
    const H = qid("demo", id("H"));
    const D = qid("demo", id("D"));
    const extra = (kind: string) =>
      graphOf(
        [id("H"), id("D"), ...Array.from({ length: 20 }, (_v, i) => id("S", i))],
        [...Array.from({ length: 20 }, (_v, i) => [edgeId(i), "supports", id("S", i), id("H")] as [string, string, string, string]), [edgeId(99), kind, id("D"), id("H")]],
      );
    const thread: Entities = { list: [], of: new Map([[D, 0], [H, 0]]), proposed: [] };
    for (const [g, r] of [
      [extra("not_same_as"), rules()],
      [extra("same_as"), rules({ entities: thread })],
      [extra("possibly_same_as"), rules({ proposedPairs: new Set([`${D}|${H}`, `${H}|${D}`]) })],
    ] as const) {
      const shown = forcedShown(g, new Set(), null, edgeLeftOut(g, r));
      // The denial's own end selected: nothing to label. The hub selected, capped at 20: the left-out edge takes no slot.
      expect(forcedLabels(g, { selected: D, hovered: null, focusCentre: null }, shown).edges.size).toBe(0);
      const hub = forcedLabels(g, { selected: H, hovered: D, focusCentre: null }, shown, 20);
      expect([hub.edges.size, hub.more, [...hub.edges].some((e) => e.endsWith(edgeId(99)))]).toEqual([20, 0, false]);
    }
    // Drawn, the same edge is labelled and counted.
    const g = extra("supports");
    const hub = forcedLabels(g, { selected: H, hovered: null, focusCentre: null }, forcedShown(g, new Set(), null, edgeLeftOut(g, rules())), 20);
    expect([hub.edges.size, hub.more]).toEqual([20, 1]);
  });

  it("draws a selected node's edges in each relation's colour in Console, the focus colour in Light, ink in Instrument", () => {
    const palette = { ink: "#ink", edgeFocus: "#amber", edgeStructure: "#structure" };
    const blocks = selectedEdgeColor("blocks", { ...palette, theme: "console" });
    expect(blocks).not.toBe("#amber");
    expect(blocks).not.toBe(selectedEdgeColor("supports", { ...palette, theme: "console" }));
    expect(selectedEdgeColor("part_of", { ...palette, theme: "console" })).toBe("#structure");
    expect(selectedEdgeColor("blocks", { ...palette, theme: "light" })).toBe("#amber");
    expect(selectedEdgeColor("blocks", { ...palette, theme: "instrument" })).toBe("#ink");
  });
});

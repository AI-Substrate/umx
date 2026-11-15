// Review round 1 (plan 133): graph-copy identity (B1), selection removals across graphs in one poll (B4),
// camera requests that keep their graph (B5), live state per graph (B6) and edge accounting kept true live (B7).
import { describe, expect, it } from "vitest";
import { type CameraAsk, cameraMove, canvasFit, fitTargets, LEDGER0, qualifyEvent, takeBatch } from "@/lib/agent";
import { applyRows, applySnapshot } from "@/lib/graph/apply";
import { buildGraph, externalCount, neighbourLinks, omittedAfter, omittedOf, openByGraphId } from "@/lib/graph/build";
import { qid } from "@/lib/graph/keys";
import { liveAll } from "@/lib/live";
import { noSelection, noticeAfterPrune, noticeShown, pickOne, type RemovalNotice, removeFromSelection, type Selection } from "@/lib/selection";
import type { ExternalRow, Neighbour, NodeRow, Snapshot } from "@/lib/types";
import type { AgentEvent, UiCommand } from "@/lib/umx/socket";

const id = (c: string) => `01${c.repeat(24)}`;
const [A, B, C] = ["A", "B", "C"].map(id);
const [GA, GB, GC] = ["1", "2", "3"].map(id);
const [E1, E2, E3] = ["E", "F", "G"].map((c) => `01EDGE${c.repeat(20)}`);

function snap(graph: string, graphId: string, nodes: string[], external: ExternalRow[] = [], broken: string[] = []): Snapshot {
  const rows: NodeRow[] = nodes.map((n) => [n, "risk", `${graph} ${n.slice(2, 3)}`, "open"]);
  return { graph, graphId, nodes: rows, edges: [], external, broken, totalNodes: rows.length, totalEdges: 0, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 } };
}

describe("B1: two open copies of one graph keep their own identity", () => {
  // alpha and alpha_copy are copies (graph id GA); alpha_copy's own edge E1 runs from its A to gamma's C, and gamma's
  // E2 points into GA, which is open twice: which copy it means is never guessed.
  const alpha = () => snap("alpha", GA, [A, B]);
  const copy = (rel = "supports") => snap("alpha_copy", GA, [A, B], [[E1, rel, GA, A, GC, C, false]]);
  const gamma = () => snap("gamma", GC, [C], [[E2, "blocks", GC, C, GA, B, false]]);

  it("draws a copy's external edge from the copy's own node, and leaves an end into a doubly open id off, counted", () => {
    const built = buildGraph([alpha(), copy(), gamma()]);
    const g = built.graph;
    expect([g.source(qid("alpha_copy", E1)), g.target(qid("alpha_copy", E1))]).toEqual([qid("alpha_copy", A), qid("gamma", C)]);
    expect(g.hasEdge(qid("gamma", E2))).toBe(false);
    expect(built.external).toEqual({ drawn: 1, notOpen: 0, broken: 1 });
  });

  it("keeps the copy's own end on overflow too", () => {
    const g = buildGraph([alpha(), copy(), gamma()]).graph;
    const out = applySnapshot(g, copy("depends_on"), openByGraphId([alpha(), copy(), gamma()]));
    expect([g.getEdgeAttribute(qid("alpha_copy", E1), "kind"), g.source(qid("alpha_copy", E1))]).toEqual(["depends_on", qid("alpha_copy", A)]);
    expect(out.edges.edited).toBe(1);
  });

  it("moves the panel to a neighbour in the selected node's own copy, and never guesses a copy for another graph's end", () => {
    const open = openByGraphId([alpha(), copy(), gamma()]);
    const n = (graph: string, local: boolean): Neighbour => ({ kind: "node", way: "out", relation: "r", edge: "e", id: B, graph, graphAlias: null, local, type: "risk", title: "t", status: "open" });
    const fromCopy = neighbourLinks(open, "alpha_copy", new Set());
    expect(fromCopy.target(n(GA, true))).toBe(qid("alpha_copy", B));
    expect(fromCopy.graphName(n(GA, true))).toBe("alpha_copy");
    const fromGamma = neighbourLinks(open, "gamma", new Set());
    expect(fromGamma.target(n(GA, false))).toBeNull();
    expect(fromGamma.graphName(n(GA, false))).toBe("(open in several copies)");
    expect(neighbourLinks(open, "gamma", new Set([GB])).graphName(n(GB, false))).toBe("(secured)");
  });
});

describe("B4: removals from two graphs in one poll compose", () => {
  it("never restores a node the first graph's removal took out", () => {
    const now: { current: Selection } = { current: { graph: "alpha,beta", ids: [qid("alpha", A), qid("beta", B)], primary: qid("alpha", A) } };
    const first = removeFromSelection(now, new Set([qid("alpha", A)]));
    expect(first.primaryGone).toBe(true);
    // beta's apply runs before React commits alpha's result: it starts from alpha's result, not the old selection.
    const second = removeFromSelection(now, new Set([qid("beta", B)]));
    expect(second.next).toEqual({ graph: "alpha,beta", ids: [], primary: null });
    expect(now.current.ids).toEqual([]);
    expect(removeFromSelection({ current: noSelection("x") }, new Set()).next).toEqual(noSelection("x"));
  });

  it("keeps the panel's \"this node was removed\" through the second graph's removal; a person's change dismisses it (round 2)", () => {
    const now: { current: Selection } = { current: { graph: "alpha,beta", ids: [qid("alpha", A), qid("beta", B)], primary: qid("alpha", A) } };
    // As graph-app applies each graph's change: prune, then compose the notice in order.
    let notice: RemovalNotice = null;
    notice = noticeAfterPrune(notice, removeFromSelection(now, new Set([qid("alpha", A)])));
    notice = noticeAfterPrune(notice, removeFromSelection(now, new Set([qid("beta", B)])));
    // The panel reads the committed selection, which is the last pruning's result.
    expect(noticeShown(notice, now.current)).toBe(qid("alpha", A));
    // A pruning that changes nothing keeps it; the person's click elsewhere dismisses it.
    notice = noticeAfterPrune(notice, removeFromSelection(now, new Set([qid("beta", C)])));
    expect(noticeShown(notice, now.current)).toBe(qid("alpha", A));
    const clicked = pickOne(now.current, qid("beta", C));
    expect(noticeShown(notice, clicked)).toBeNull();
    // After the person's change, a later pruning never brings the old notice back; removing their new node is its
    // own notice.
    const later = { current: { ...clicked, ids: [...clicked.ids, qid("beta", B)] } };
    expect(noticeShown(noticeAfterPrune(notice, removeFromSelection(later, new Set([qid("beta", B)]))), later.current)).toBeNull();
    expect(noticeShown(noticeAfterPrune(notice, removeFromSelection(later, new Set([qid("beta", C)]))), later.current)).toBe(qid("beta", C));
  });
});

describe("B5: camera requests keep their graph", () => {
  let seq = 0;
  const ev = (graph: string, command: UiCommand): AgentEvent => qualifyEvent({ seq: ++seq, at_ms: Date.now(), graph, by: null, command });
  const base = { ledger: LEDGER0, filterFrom: null, pending: null, prompt: null };

  it("a clear in beta in the same batch does not drop alpha's focus", () => {
    const out = takeBatch([ev("alpha", { kind: "focus", node: A, hops: 1 }), ev("beta", { kind: "clear" })], base);
    expect(out.pending).toMatchObject({ kind: "focus", graph: "alpha", nodes: [qid("alpha", A)] });
    expect(takeBatch([ev("alpha", { kind: "focus", node: A, hops: 1 }), ev("alpha", { kind: "clear" })], base).pending).toBeNull();
  });

  it("a clear in beta leaves alpha's waiting prompt; a clear in alpha takes it", () => {
    const prompt: CameraAsk = { seq: 1, at_ms: 1, kind: "focus", nodes: [qid("alpha", A)], graph: "alpha" };
    expect(takeBatch([ev("beta", { kind: "clear" })], { ...base, prompt }).prompt).toBe(prompt);
    expect(takeBatch([ev("alpha", { kind: "clear" })], { ...base, prompt }).prompt).toBeNull();
    // A beta filter is not taken off by alpha's clear.
    const filtered = takeBatch([ev("beta", { kind: "filter", show: [], hide: ["type:risk"] })], base);
    const after = takeBatch([ev("alpha", { kind: "clear" })], filtered);
    expect(after.ledger.filters.types.has("risk")).toBe(true);
  });

  it("an empty fit fits its own graph's shown nodes, not the whole canvas", () => {
    const fit = (graph: string): CameraAsk => ({ seq: 1, at_ms: 1, kind: "fit", nodes: [], graph });
    const nodes = [
      { key: qid("alpha", A), graph: "alpha", hidden: false },
      { key: qid("alpha", B), graph: "alpha", hidden: true },
      { key: qid("beta", A), graph: "beta", hidden: false },
    ];
    expect(fitTargets(fit("alpha"), nodes, 2)).toEqual([qid("alpha", A)]);
    expect(fitTargets(fit("alpha"), nodes, 1)).toEqual([]);
    expect(fitTargets({ ...fit("alpha"), nodes: [qid("alpha", B)] }, nodes, 2)).toEqual([qid("alpha", B)]);
  });

  it("leaves the camera alone for a graph-named fit with none of its nodes shown, beside a shown graph (round 2)", () => {
    const fit = (graph: string): CameraAsk => ({ seq: 1, at_ms: 1, kind: "fit", nodes: [], graph });
    const shown = new Set([qid("beta", A)]);
    const nodes = [
      { key: qid("alpha", A), graph: "alpha", hidden: true },
      { key: qid("beta", A), graph: "beta", hidden: false },
    ];
    // Dispatch (follow mode, prompt Go and replay share it): nothing for alpha, so the canvas never sees a reset.
    expect(cameraMove(fit("alpha"), nodes, 2)).toBeNull();
    expect(cameraMove(fit("gamma"), nodes, 2)).toBeNull();
    expect(cameraMove(fit("beta"), nodes, 2)).toEqual({ kind: "fit", nodes: [qid("beta", A)] });
    // One graph open: an empty fit is still the plain reset, and the canvas resets.
    const one = cameraMove(fit("alpha"), nodes, 1);
    expect(one).toEqual({ kind: "fit", nodes: [] });
    expect(canvasFit(one!.kind === "fit" ? one!.nodes : [qid("x", A)], (id) => shown.has(id))).toBe("reset");
    // The canvas: named nodes none of which are shown stay put; shown ones are fitted.
    expect(canvasFit([qid("alpha", A)], (id) => shown.has(id))).toBe("stay");
    expect(canvasFit([qid("alpha", A), qid("beta", A)], (id) => shown.has(id))).toEqual([qid("beta", A)]);
    expect(cameraMove({ seq: 1, at_ms: 1, kind: "focus", nodes: [qid("alpha", A)], graph: "alpha" }, nodes, 2)).toEqual({ kind: "jump", id: qid("alpha", A) });
  });
});

describe("B6: live only when every open graph is current", () => {
  it("one graph's success never hides another's failure", () => {
    const at = { epoch: "00000000000000e1", rev: 3 };
    const failed = new Map([["alpha", { code: "x", message: "rows failed" }]]);
    expect(liveAll([at, at], failed, ["alpha", "beta"])).toBe(false);
    expect(liveAll([at, at], new Map(), ["alpha", "beta"])).toBe(true);
    expect(liveAll([at, null], new Map(), ["alpha", "beta"])).toBe(false);
    // A failure in a graph that left the canvas no longer counts.
    expect(liveAll([at], failed, ["beta"])).toBe(true);
  });
});

describe("B7: the counts follow live changes and overflow", () => {
  it("counts a link added or removed live, one into a graph not open, and rereads them on overflow", () => {
    const alpha = snap("alpha", GA, [A, B], [[E1, "supports", GA, A, GB, A, false]], [E3]);
    const beta = snap("beta", GB, [A]);
    const built = buildGraph([alpha, beta]);
    const g = built.graph;
    let om = built.omitted;
    expect(built.external).toEqual({ drawn: 1, notOpen: 0, broken: 1 });
    // A second link alpha/B -> beta/A added live, and an edge into gamma (not open) that comes back undrawable.
    applyRows(g, { nodes: [], edges: [[qid("alpha", E2), "supports", qid("alpha", B), qid("beta", A)]] }, { nodes: [], edges: [] });
    om = new Map(om).set("alpha", omittedAfter(om.get("alpha"), g, [qid("alpha", E2), qid("alpha", id("Q"))], []));
    expect(externalCount(g, om.values())).toEqual({ drawn: 2, notOpen: 1, broken: 1 });
    // E1 removed live, and the broken edge too.
    applyRows(g, { nodes: [], edges: [] }, { nodes: [], edges: [qid("alpha", E1)] });
    om = new Map(om).set("alpha", omittedAfter(om.get("alpha"), g, [], [qid("alpha", E1), qid("alpha", E3)]));
    expect(externalCount(g, om.values())).toEqual({ drawn: 1, notOpen: 1, broken: 0 });
    // Overflow: alpha read again with only E1 (back) and nothing else.
    const fresh = snap("alpha", GA, [A, B], [[E1, "supports", GA, A, GB, A, false]]);
    const open = openByGraphId([fresh, beta]);
    applySnapshot(g, fresh, open);
    om = new Map(om).set("alpha", omittedOf(g, fresh, open));
    expect(externalCount(g, om.values())).toEqual({ drawn: 1, notOpen: 0, broken: 0 });
  });
});

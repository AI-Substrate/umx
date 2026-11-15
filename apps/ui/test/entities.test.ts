// Entities across the open graphs (ws009 M2, plan 165): umx's rule (a shared key within one type, a same_as link,
// onward), proposed partners never joined, one copy per graph id, links through graphs that are not open, and the
// merged panel's field rule (D69 (2)).
import { describe, expect, it } from "vitest";
import { buildGraph } from "@/lib/graph/build";
import { applyNudge, applyRows, nudgeInput, snapAfter } from "@/lib/graph/apply";
import { applyMerge, coveredOf, entitiesOf, entityOf, HIDDEN_KEY, homeOf, identityEdgeHidden, keyShown, mergedPlaces, mergeRecords, NO_ENTITIES, throughLinks, threadPairs, worldOf } from "@/lib/graph/entities";
import { loadSnapshot } from "@/lib/snapshot";
import { qid } from "@/lib/graph/keys";
import type { EdgeRow, ExternalRow, NodeRow, Snapshot } from "@/lib/types";

const id = (i: number) => `01${String(i).padStart(24, "0")}`;
const [GA, GB, GC, GS] = ["01GRAPHAAAAAAAAAAAAAAAAAAA", "01GRAPHBBBBBBBBBBBBBBBBBBB", "01GRAPHCCCCCCCCCCCCCCCCCCC", "01GRAPHSSSSSSSSSSSSSSSSSSS"];

function snap(graph: string, graphId: string, nodes: NodeRow[], edges: EdgeRow[] = [], external: ExternalRow[] = []): Snapshot {
  return { graph, graphId, nodes, edges, external, totalNodes: nodes.length, totalEdges: edges.length, truncated: false, omittedEdges: { otherGraph: external.length, unresolved: 0 } };
}

const person = (i: number, keys: string[] = [], kind = "person"): NodeRow => [id(i), kind, `p${i}`, "open", "2026-11-01T00:00:00Z", keys];

/** The view's world over `snaps` (open in this order). */
function world(snaps: Snapshot[]) {
  const { graph } = buildGraph(snaps);
  const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
  const covered = coveredOf(snaps);
  return { graph, graphIds, w: worldOf(graph, graphIds, covered, throughLinks(snaps, graph)) };
}

const members = (v: ReturnType<typeof world>, key: string) => entityOf(v.w, key, v.graphIds)?.members.map((m) => m.key) ?? null;

describe("the entity builder follows umx's rule", () => {
  it("joins a shared key within one type only, a same_as link, and onward through both", () => {
    const alpha = snap("alpha", GA, [person(1, ["email:a@example.invalid"]), person(2, ["email:a@example.invalid"], "company")]);
    const beta = snap("beta", GB, [person(1, ["email:a@example.invalid", "crm:7"])]);
    const gamma = snap("gamma", GC, [person(3, ["crm:7"]), person(4)], [], [[id(50), "same_as", GC, id(4), GB, id(1), false]]);
    const v = world([alpha, beta, gamma]);
    // alpha 1 -key-> beta 1 -key crm:7-> gamma 3; gamma 4 same_as beta 1. alpha 2 is a company: never joined.
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1)), qid("beta", id(1)), qid("gamma", id(3)), qid("gamma", id(4))]);
    const shown = entityOf(v.w, qid("gamma", id(4)), v.graphIds)!;
    expect(shown.members[0].matched).toContainEqual({ by: "asked" });
    expect(shown.members.find((m) => m.key === qid("beta", id(1)))!.matched).toEqual([
      { by: "key", key: "email:a@example.invalid" },
      { by: "key", key: "crm:7" },
      { by: "link", graph: "gamma", edge: qid("gamma", id(50)) },
    ]);
    expect(members(v, qid("alpha", id(2)))).toEqual([qid("alpha", id(2))]);
    const all = entitiesOf(v.w);
    expect(all.list).toHaveLength(1);
    // gamma holds two members (3 by key, 4 by link): umx warns entity_graph_twice.
    expect(all.list[0]).toMatchObject({ graphs: 3, twice: ["gamma"] });
    expect(all.of.has(qid("alpha", id(2)))).toBe(false);
  });

  it("lists a possibly_same_as partner as proposed and never joins or merges it", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"]), person(2)], [[id(60), "possibly_same_as", id(1), id(2)]]);
    const beta = snap("beta", GB, [person(1, ["k:1"])]);
    const v = world([alpha, beta]);
    const shown = entityOf(v.w, qid("alpha", id(1)), v.graphIds)!;
    expect(shown.members.map((m) => m.key)).toEqual([qid("alpha", id(1)), qid("beta", id(1))]);
    expect(shown.proposed).toEqual([{ at: `${GA}/${id(2)}`, key: qid("alpha", id(2)), graph: "alpha", with: qid("alpha", id(1)), edge: qid("alpha", id(60)), graph_of_edge: "alpha" }]);
    const all = entitiesOf(v.w);
    expect(all.of.has(qid("alpha", id(2)))).toBe(false);
    expect(all.proposed).toEqual([[qid("alpha", id(1)), qid("alpha", id(2))]]);
  });

  it("never makes an entity of a node with itself across two copies of one graph", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"])]);
    const copy = snap("alpha2", GA, [person(1, ["k:1"])]);
    const v = world([alpha, copy]);
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1))]);
    expect(entityOf(v.w, qid("alpha2", id(1)), v.graphIds)).toBeNull();
    expect(entitiesOf(v.w).list).toEqual([]);
  });

  it("walks through a node in a graph that is not open, by the links the open graphs store, and never shows it", () => {
    // alpha 1 same_as S:9 (stored in alpha) and beta 1 same_as S:9 (stored in beta): S is not open.
    const alpha = snap("alpha", GA, [person(1)], [], [[id(70), "same_as", GA, id(1), GS, id(9), true]]);
    const beta = snap("beta", GB, [person(1)], [], [[id(71), "same_as", GB, id(1), GS, id(9), true]]);
    const v = world([alpha, beta]);
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1)), qid("beta", id(1))]);
    expect(entitiesOf(v.w).list[0].members.sort()).toEqual([qid("alpha", id(1)), qid("beta", id(1))]);
    // A chain S stores itself (S:9 same_as beta 1, stored in S) is not read while S is not open: the same as unloaded.
    const lone = world([snap("alpha", GA, [person(1)], [], [[id(70), "same_as", GA, id(1), GS, id(9), true]]), snap("beta", GB, [person(1)])]);
    expect(members(lone, qid("alpha", id(1)))).toEqual([qid("alpha", id(1))]);
  });

  it("joins nothing by a key two nodes of one type hold in one graph, either way (ws008 E2b)", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"]), person(2, ["k:1"])]);
    const beta = snap("beta", GB, [person(3, ["k:1"]), person(4, ["k:1"], "company")]);
    const v = world([alpha, beta]);
    expect(members(v, qid("beta", id(3)))).toEqual([qid("beta", id(3))]);
    expect(members(v, qid("alpha", id(2)))).toEqual([qid("alpha", id(2))]);
    expect(entitiesOf(v.w).list).toEqual([]);
    // Held once in each graph, it joins; one more holder in beta and it joins nothing there.
    const once = world([snap("alpha", GA, [person(1, ["k:1"])]), snap("beta", GB, [person(3, ["k:1"])])]);
    expect(members(once, qid("alpha", id(1)))).toEqual([qid("alpha", id(1)), qid("beta", id(3))]);
  });

  it("follows a live change: a new same_as joins, a key removed leaves", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"])]);
    const beta = snap("beta", GB, [person(1, ["k:1"]), person(2)]);
    const snaps = [alpha, beta];
    const { graph } = buildGraph(snaps);
    const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
    const again = () => entitiesOf(worldOf(graph, graphIds, coveredOf(snaps))).list.map((e) => e.members.sort());
    expect(again()).toEqual([[qid("alpha", id(1)), qid("beta", id(1))]]);
    applyRows(graph, { nodes: [], edges: [[qid("beta", id(80)), "same_as", qid("beta", id(2)), qid("alpha", id(1))]] }, { nodes: [], edges: [] });
    expect(again()).toEqual([[qid("alpha", id(1)), qid("beta", id(1)), qid("beta", id(2))]]);
    applyRows(graph, { nodes: [[qid("beta", id(1)), "person", "p1", "open", "2026-11-02T00:00:00Z", []]], edges: [] }, { nodes: [], edges: [] });
    expect(again()).toEqual([[qid("alpha", id(1)), qid("beta", id(2))]]);
  });
});

describe("denials split entities in umx's fixed order (ws008 E2c, ws009 M2b)", () => {
  const sorted = (list: string[] | null) => (list ? [...list].sort() : list);
  const groups = (v: ReturnType<typeof world>) => entitiesOf(v.w).list.map((e) => [...e.members].sort()).sort();
  const deny = (n: number, from: string, to: string, fromGraph: string, toGraph: string): ExternalRow => [id(n), "not_same_as", fromGraph, from, toGraph, to, false];

  it("splits a pair a key joins, lists the denial, and joins them again once it is gone (a_denial_splits_an_entity)", () => {
    const alpha = snap("alpha", GA, [person(1, ["email:jo@example.invalid"])], [], [deny(90, id(1), id(2), GA, GB)]);
    const beta = snap("beta", GB, [person(2, ["email:jo@example.invalid"])]);
    const v = world([alpha, beta]);
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1))]);
    expect(groups(v)).toEqual([]);
    const shown = entityOf(v.w, qid("alpha", id(1)), v.graphIds)!;
    expect(shown.denied).toEqual([{ at: `${GB}/${id(2)}`, key: qid("beta", id(2)), graph: "beta", with: qid("alpha", id(1)), edge: qid("alpha", id(90)), graph_of_edge: "alpha" }]);
    const without = world([snap("alpha", GA, [person(1, ["email:jo@example.invalid"])]), beta]);
    expect(groups(without)).toEqual([[qid("alpha", id(1)), qid("beta", id(2))]]);
  });

  it("anchors a key at its first holder by graph id: A joins C and B stays apart, from every member (denials_split_in_one_fixed_order)", () => {
    const key = ["email:jo@example.invalid"];
    // Opened in reverse graph-id order: the order is umx's (graph id), not the open order.
    const gamma = snap("gamma", GC, [person(3, key)]);
    const beta = snap("beta", GB, [person(2, key)]);
    const alpha = snap("alpha", GA, [person(1, key)], [], [deny(90, id(1), id(2), GA, GB)]);
    const v = world([gamma, beta, alpha]);
    const ac = [qid("alpha", id(1)), qid("gamma", id(3))];
    expect(sorted(members(v, qid("alpha", id(1))))).toEqual(ac);
    expect(sorted(members(v, qid("gamma", id(3))))).toEqual(ac);
    expect(members(v, qid("beta", id(2)))).toEqual([qid("beta", id(2))]);
    expect(groups(v)).toEqual([ac]);
  });

  it("joins no target by a key held twice in a graph: C and D are one, A alone (review R4, a reviewer's A/B/C/D)", () => {
    const alpha = snap("alpha", GA, [person(1, ["domain:a.example", "email:b@example.invalid"]), person(5, ["domain:a.example"])], [], [deny(90, id(1), id(4), GA, GC)]);
    const beta = snap("beta", GB, [person(3, ["domain:a.example", "name:c"])]);
    const gamma = snap("gamma", GC, [person(4, ["email:b@example.invalid", "name:c"])]);
    const v = world([alpha, beta, gamma]);
    const cd = [qid("beta", id(3)), qid("gamma", id(4))];
    expect(sorted(members(v, qid("beta", id(3))))).toEqual(cd);
    expect(sorted(members(v, qid("gamma", id(4))))).toEqual(cd);
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1))]);
    expect(groups(v)).toEqual([cd]);
  });

  it("ignores a denial with an end in a graph that is not open, and one a graph that is not covered stores (a_denial_with_an_uncovered_node)", () => {
    // A and B both linked to S:9 (not open) by links beta stores; alpha stores a denial of A and S:9.
    const alpha = snap("alpha", GA, [person(1)], [], [deny(90, id(1), id(9), GA, GS)]);
    const beta = snap("beta", GB, [person(2)], [], [[id(70), "same_as", GA, id(1), GS, id(9), false], [id(71), "same_as", GB, id(2), GS, id(9), true]]);
    const v = world([alpha, beta]);
    expect(members(v, qid("alpha", id(1)))).toEqual([qid("alpha", id(1)), qid("beta", id(2))]);
    expect(entityOf(v.w, qid("alpha", id(1)), v.graphIds)!.denied).toEqual([]);
    // A second copy of alpha is not covered (d-21): the denial it stores counts no more than any of its edges.
    const keyed = snap("alpha", GA, [person(1, ["k:1"])]);
    const copy = snap("alpha2", GA, [person(1, ["k:1"])], [], [deny(91, id(1), id(2), GA, GB)]);
    const two = world([keyed, snap("beta", GB, [person(2, ["k:1"])]), copy]);
    expect(members(two, qid("alpha", id(1)))).toEqual([qid("alpha", id(1)), qid("beta", id(2))]);
  });

  it("drops a proposal into a group the entity is denied with, even one the walk never reached (review R5)", () => {
    const key = ["email:jo@example.invalid"];
    const alphaNodes = [person(1)];
    const beta = snap("beta", GB, [person(2, key)]);
    const gamma = snap("gamma", GC, [person(3, key)], [], [[id(60), "possibly_same_as", GA, id(1), GC, id(3), false]]);
    const control = world([snap("alpha", GA, alphaNodes), beta, gamma]);
    expect(entityOf(control.w, qid("alpha", id(1)), control.graphIds)!.proposed.map((p) => p.key)).toEqual([qid("gamma", id(3))]);
    expect(entitiesOf(control.w).proposed).toHaveLength(1);
    const v = world([snap("alpha", GA, alphaNodes, [], [deny(90, id(1), id(2), GA, GB)]), beta, gamma]);
    expect(entityOf(v.w, qid("alpha", id(1)), v.graphIds)!.proposed).toEqual([]);
    expect(entitiesOf(v.w).proposed).toEqual([]);
    // The partner's own read drops it too.
    expect(entityOf(v.w, qid("gamma", id(3)), v.graphIds)!.proposed).toEqual([]);
  });

  it("orders keys as umx does (code points, not UTF-16 units)", () => {
    // "\uff5e" sorts before "\u{1f600}" by code point, after it by UTF-16 unit. A and C are denied: the first key
    // joined decides which of them B joins.
    const [k1, k2] = ["x:\uff5e", "x:\u{1f600}"];
    const alpha = snap("alpha", GA, [person(1, [k1])], [], [deny(90, id(1), id(3), GA, GC)]);
    const beta = snap("beta", GB, [person(2, [k1, k2])]);
    const gamma = snap("gamma", GC, [person(3, [k2])]);
    const v = world([alpha, beta, gamma]);
    expect(groups(v)).toEqual([[qid("alpha", id(1)), qid("beta", id(2))]]);
  });

  it("never draws a denial as an edge, and draws an identity edge only where no thread does (ui-expert N1)", () => {
    const [a, b, c] = [qid("alpha", id(1)), qid("beta", id(2)), qid("beta", id(3))];
    const entities = { list: [{ members: [a, b], graphs: 2, twice: [] }], of: new Map([[a, 0], [b, 0]]), proposed: [[a, c]] as [string, string][] };
    const pairs = new Set([`${a}|${c}`, `${c}|${a}`]);
    expect(identityEdgeHidden("not_same_as", a, c, entities, pairs)).toBe(true);
    expect(identityEdgeHidden("same_as", a, b, entities, pairs)).toBe(true);
    expect(identityEdgeHidden("same_as", a, c, entities, pairs)).toBe(false);
    expect(identityEdgeHidden("possibly_same_as", c, a, entities, pairs)).toBe(true);
    expect(identityEdgeHidden("possibly_same_as", b, c, entities, pairs)).toBe(false);
    expect(identityEdgeHidden("supports", a, b, entities, pairs)).toBe(false);
  });

  it("follows a live denial: added, it splits; removed, the pair joins again", () => {
    const snaps = [snap("alpha", GA, [person(1, ["k:1"])]), snap("beta", GB, [person(2, ["k:1"])])];
    const { graph } = buildGraph(snaps);
    const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
    const again = () => entitiesOf(worldOf(graph, graphIds, coveredOf(snaps))).list.length;
    expect(again()).toBe(1);
    applyRows(graph, { nodes: [], edges: [[qid("beta", id(90)), "not_same_as", qid("beta", id(2)), qid("alpha", id(1))]] }, { nodes: [], edges: [] });
    expect(again()).toBe(0);
    applyRows(graph, { nodes: [], edges: [] }, { nodes: [], edges: [qid("beta", id(90))] });
    expect(again()).toBe(1);
  });
});

describe("threads and the merged panel", () => {
  it("joins every pair of a small entity, and each member to the first past six", () => {
    expect(threadPairs(["a", "b", "c"])).toEqual([["a", "b"], ["a", "c"], ["b", "c"]]);
    expect(threadPairs("abcdefg".split(""))).toHaveLength(6);
  });

  it("takes the title and each field from the latest member that has it, keeping the others with their graph", () => {
    const merged = mergeRecords([
      { graph: "crm", key: "crm/1", node: { title: "Ada", updated: "2026-11-10T09:00:00Z", phone: "1", city: "Perth", entity_key: ["k:1"] } },
      { graph: "hr", key: "hr/1", node: { title: "Ada L", updated: "2026-11-12T09:14:00Z", phone: "2", team: null } },
      { graph: "ops", key: "ops/1", node: { title: "A. L.", updated: "2026-11-11T00:00:00Z", phone: "2", team: "infra" } },
    ])!;
    expect(merged).toMatchObject({ title: "Ada L", from: "hr", updated: "2026-11-12T09:14:00Z" });
    expect(merged.fields).toEqual([
      { name: "city", value: "Perth", graph: "crm", others: [] },
      { name: "phone", value: "2", graph: "hr", others: [{ value: "1", graph: "crm" }] },
      { name: "team", value: "infra", graph: "ops", others: [] },
    ]);
    expect(merged.fields.map((f) => f.name)).not.toContain("entity_key");
  });
});

describe("merged positions", () => {
  it("moves each shown member to its entity's centroid, keeps where it is laid out, and puts it back", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"]), person(2)]);
    const beta = snap("beta", GB, [person(1, ["k:1"])]);
    const v = world([alpha, beta]);
    const [a1, b1, a2] = [qid("alpha", id(1)), qid("beta", id(1)), qid("alpha", id(2))];
    v.graph.mergeNodeAttributes(a1, { x: 0, y: 0 });
    v.graph.mergeNodeAttributes(b1, { x: 10, y: 4 });
    v.graph.mergeNodeAttributes(a2, { x: 3, y: 3 });
    const all = entitiesOf(v.w);
    applyMerge(v.graph, all);
    expect(v.graph.getNodeAttributes(a1)).toMatchObject({ x: 5, y: 2, homeX: 0, homeY: 0 });
    expect(v.graph.getNodeAttributes(b1)).toMatchObject({ x: 5, y: 2, homeX: 10, homeY: 4 });
    expect(v.graph.getNodeAttributes(a2)).toMatchObject({ x: 3, y: 3 });
    expect(homeOf(v.graph.getNodeAttributes(b1))).toEqual({ x: 10, y: 4 });
    // Merging again (a live change, a filter) starts from where they are laid out, not from the centroid.
    applyMerge(v.graph, all);
    expect(v.graph.getNodeAttributes(b1)).toMatchObject({ x: 5, y: 2, homeX: 10 });
    applyMerge(v.graph, NO_ENTITIES);
    expect(v.graph.getNodeAttributes(b1)).toMatchObject({ x: 10, y: 4 });
    expect(v.graph.getNodeAttribute(b1, "homeX")).toBeUndefined();
  });
});

describe("review B1: a link deleted live stays deleted", () => {
  it("drops an external same_as removed live from the snapshot, so the walk and the groups split", () => {
    const alpha = snap("alpha", GA, [person(1)], [], [[id(90), "same_as", GA, id(1), GB, id(1), false]]);
    const beta = snap("beta", GB, [person(1)]);
    let snaps = [alpha, beta];
    const { graph } = buildGraph(snaps);
    const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
    const rebuild = () => worldOf(graph, graphIds, coveredOf(snaps), throughLinks(snaps, graph));
    expect(entitiesOf(rebuild()).list).toHaveLength(1);
    // The live unlink: the edge is read as gone, applied, and the snapshot updated the way the app does.
    const outcome = applyRows(graph, { nodes: [], edges: [] }, { nodes: [], edges: [qid("alpha", id(90))] });
    snaps = snaps.map((s) => (s.graph === "alpha" ? snapAfter(s, outcome, {}, [id(90)]) : s));
    expect(throughLinks(snaps, graph)).toEqual([]);
    expect(entityOf(rebuild(), qid("alpha", id(1)), graphIds)!.members.map((m) => m.key)).toEqual([qid("alpha", id(1))]);
    expect(entitiesOf(rebuild()).list).toEqual([]);
  });
});

describe("review B4: every snapshot node keeps its updated time", () => {
  it("titles a merged entity by its newest member, an unkeyed target of a link only the other graph stores", async () => {
    const GA2 = GA;
    const read = (async (verb: string, args: { graph?: string; offset?: number } = {}) => {
      const env = (data: unknown) => ({ umx: 60, command: verb, status: "ok", data, warnings: [], errors: [] });
      if (verb === "graph list") return env({ rows: [{ alias: "alpha", id: GA2, state: "loaded", folder: "present", policy: "open" }, { alias: "beta", id: GB, state: "loaded", folder: "present", policy: "open" }] });
      if (verb === "ontology show") return env({ symmetric: ["same_as"] });
      if (verb === "node list") {
        const n = args.graph === "alpha" ? { id: id(1), type: "person", title: "Old title", status: "open", updated: "2026-11-01T00:00:00Z" } : { id: id(2), type: "person", title: "New title", status: "open", updated: "2026-11-12T00:00:00Z" };
        return env({ rows: [n], total: 1, truncated: false });
      }
      // The only same_as is stored in alpha, its end in beta.
      const rows = args.graph === "alpha" ? [{ id: id(50), type: "same_as", from: id(1), to: id(2), from_graph: GA2, to_graph: GB, graph_id: GA2 }] : [];
      return env({ rows, total: rows.length, truncated: false });
    }) as unknown as Parameters<typeof loadSnapshot>[0];
    const snaps = [await loadSnapshot(read, "alpha"), await loadSnapshot(read, "beta")];
    expect(snaps[1].nodes[0][4]).toBe("2026-11-12T00:00:00Z");
    const { graph } = buildGraph(snaps);
    const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
    const all = entitiesOf(worldOf(graph, graphIds, coveredOf(snaps), throughLinks(snaps, graph)));
    const places = mergedPlaces(graph, all);
    expect(places.get(qid("alpha", id(1)))!.title).toBe("New title");
  });
});

describe("review B6: a live member settles under a merged entity", () => {
  it("places and nudges by where nodes are laid out, merges again after the nudge, and unmerges to the settled place", () => {
    const alpha = snap("alpha", GA, [person(1, ["k:1"])]);
    const beta = snap("beta", GB, [person(1, ["k:1"])]);
    const snaps = [alpha, beta];
    const { graph } = buildGraph(snaps);
    const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
    const [a1, b1, b2] = [qid("alpha", id(1)), qid("beta", id(1)), qid("beta", id(2))];
    graph.mergeNodeAttributes(a1, { x: 0, y: 0 });
    graph.mergeNodeAttributes(b1, { x: 100, y: 0 });
    const groups = () => entitiesOf(worldOf(graph, graphIds, coveredOf(snaps)));
    applyMerge(graph, groups());
    // A live node joined by same_as and linked to beta's member: placed near where its neighbours are laid out, not at
    // the centroid they are drawn at. (A second k:1 holder in beta would join nothing: ws008 E2b.)
    const out = applyRows(
      graph,
      { nodes: [[b2, "person", "p2", "open", "2026-11-02T00:00:00Z", []]], edges: [[qid("beta", id(70)), "supports", b2, b1], [qid("beta", id(71)), "same_as", b2, b1]] },
      { nodes: [], edges: [] },
    );
    expect(out.fresh).toEqual([b2]);
    expect(Math.abs(graph.getNodeAttribute(b2, "x") - 100)).toBeLessThan(60);
    applyMerge(graph, groups());
    const made = nudgeInput(graph, [b2])!;
    const at = made.ids.indexOf(b1) * 2;
    expect(made.input.positions[at]).toBe(100);
    // The worker settles it at (90, 20).
    const positions = Float32Array.from(made.input.positions);
    positions[made.ids.indexOf(b2) * 2] = 90;
    positions[made.ids.indexOf(b2) * 2 + 1] = 20;
    applyNudge(graph, made.ids, made.input.fixed, positions);
    applyMerge(graph, groups());
    const xy = (k: string) => [graph.getNodeAttribute(k, "x"), graph.getNodeAttribute(k, "y")];
    expect(xy(b2)).toEqual(xy(a1));
    expect(xy(b1)).toEqual(xy(a1));
    expect(xy(a1)[0]).toBeCloseTo((0 + 100 + 90) / 3);
    applyMerge(graph, NO_ENTITIES);
    expect(xy(b2)).toEqual([90, 20]);
    expect(xy(b1)).toEqual([100, 0]);
  });
});

describe("keys in the panel: shown unless umx flags them (security and M2b)", () => {
  it("shows a key the entity read gave unflagged, and any other as its plain scheme and (hidden key)", async () => {
    const { joinedText } = await import("@/components/entity-section");
    expect(keyShown("email:ada@example.invalid")).toBe("email");
    expect(keyShown("Weird-Scheme:x")).toBe(HIDDEN_KEY);
    expect(keyShown("novalue")).toBe(HIDDEN_KEY);
    const matched = [{ by: "asked" }, { by: "key", key: "email:ada@example.invalid" }, { by: "key", key: "crm:7" }, { by: "key", key: "Bad:x" }, { by: "link", graph: "ops", edge: "ops/1" }] as const;
    // Before the entity read came, and for a key it flagged (sent as "email:(hidden key)"), no value is shown.
    for (const shown of [null, ["email:(hidden key)"]]) {
      const text = joinedText(matched, shown);
      expect(text).toBe("this node · by key (email:(hidden key), crm:(hidden key), (hidden key)) · linked in ops");
      expect(text).not.toContain("example.invalid");
    }
    // The read gave crm:7 unflagged: by value, it is shown.
    expect(joinedText(matched, ["email:(hidden key)", "crm:7"])).toBe("this node · by key (email:(hidden key), crm:7, (hidden key)) · linked in ops");
  });
});

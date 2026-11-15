// Shadow connections (ws009 M1c, plan 169): pairs once at the higher score, which may be drawn (round 1: the
// current visibility, merged real edges, exclusions before the cap), the server read (open graphs only, ids and scores,
// one call per model key, stops when the browser goes, keeps answers only while every vectors_digest stands) and who
// owns a read (only the newest lands, Refresh always asks, Hold reads only what is applied).
import { describe, expect, it, vi } from "vitest";
import { buildGraph, shownMask } from "@/lib/graph/build";
import { applyMerge, coveredOf, entitiesOf, mergedPlaces, worldOf } from "@/lib/graph/entities";
import { NO_FILTERS } from "@/lib/graph/filters";
import { qid } from "@/lib/graph/keys";
import { allowed, allShadows, farEnd, forScope, indexPairs, mergeShadowReads, nodeShadows, type Pair, pairsOf, shadowAlpha, shadowScope, shownBy, skippedLegend, skippedPanel } from "@/lib/graph/shadows";
import { ShadowReads, type ShadowState } from "@/lib/shadow-reads";
import { CACHE_ENTRIES, CACHE_ROWS, loadShadows, ShadowCache, type ShadowReply, type ShadowRow } from "@/lib/shadows-read";
import { type createUmx, UmxError } from "@/lib/umx";
import type { EdgeRow, NodeRow, Snapshot } from "@/lib/types";

const id = (i: number) => `01${String(i).padStart(24, "0")}`;
const [GA, GB] = ["01GRAPHAAAAAAAAAAAAAAAAAAA", "01GRAPHBBBBBBBBBBBBBBBBBBB"];
const node = (i: number, keys: string[] = [], kind = "risk"): NodeRow =>
  keys.length ? [id(i), kind, `n${i}`, "open", "2026-11-01T00:00:00Z", keys] : [id(i), kind, `n${i}`, "open", "2026-11-01T00:00:00Z"];
const snap = (graph: string, graphId: string, nodes: NodeRow[], edges: EdgeRow[] = []): Snapshot => ({
  graph, graphId, nodes, edges, external: [], totalNodes: nodes.length, totalEdges: edges.length, truncated: false, omittedEdges: { otherGraph: 0, unresolved: 0 },
});
const [a1, a2, a3, b1, b2] = [qid("alpha", id(1)), qid("alpha", id(2)), qid("alpha", id(3)), qid("beta", id(1)), qid("beta", id(2))];
const P = (a: string, b: string, score: number): Pair => (a < b ? { a, b, score, floor: 0.8 } : { a: b, b: a, score, floor: 0.8 });

function canvas(alpha: NodeRow[], alphaEdges: EdgeRow[], beta: NodeRow[], betaEdges: EdgeRow[] = []) {
  const snaps = [snap("alpha", GA, alpha, alphaEdges), snap("beta", GB, beta, betaEdges)];
  const { graph } = buildGraph(snaps);
  const graphIds = new Map(snaps.map((s) => [s.graph, s.graphId]));
  const entities = entitiesOf(worldOf(graph, graphIds, coveredOf(snaps)));
  return { graph, entities, shown: shownBy(graph, null) };
}
const plain = () => canvas([node(1), node(2), node(3, ["k:1"])], [[id(9), "supports", id(1), id(2)]], [node(1, [], "issue"), node(2, ["k:1"])]);

describe("shadow pairs", () => {
  it("keeps each pair once, undirected, at the higher score, with its model's floor", () => {
    const rows: ShadowRow[] = [["alpha", id(1), "beta", id(1), 0.9, 0], ["beta", id(1), "alpha", id(1), 0.93, 0], ["alpha", id(1), "alpha", id(3), 0.88, 1]];
    expect(pairsOf(rows, [{ floor: 0.85 }, { floor: null }])).toEqual([{ a: a1, b: b1, score: 0.93, floor: 0.85 }, { a: a1, b: a3, score: 0.88, floor: null }]);
  });

  it("never draws one between linked nodes, members of one entity, hidden nodes or nodes not on the canvas", () => {
    const ctx = plain();
    expect(allowed(P(a1, b1, 0.9), ctx)).toBe(true);
    expect(allowed(P(a1, a2, 0.9), ctx)).toBe(false);
    expect(allowed(P(a3, b2, 0.9), ctx)).toBe(false);
    expect(allowed(P(a1, qid("gamma", id(1)), 0.9), ctx)).toBe(false);
  });

  it("takes the current visibility, before the canvas marks nodes hidden, on hide and on reveal (review B1)", () => {
    const ctx = plain();
    const index = indexPairs([P(a1, b1, 0.95)]);
    const off = { ...NO_FILTERS, types: new Set(["issue"]) };
    const at = (filters: typeof NO_FILTERS) => ({ ...ctx, shown: shownBy(ctx.graph, shownMask(ctx.graph, { provenance: false, retired: false }, filters)) });
    // The filter just turned beta's issue off; its hidden attribute is still false.
    expect(ctx.graph.getNodeAttribute(b1, "hidden")).toBe(false);
    expect(nodeShadows(index, at(off), a1, 5)).toEqual([]);
    expect(allShadows(index, at(off)).pairs).toEqual([]);
    // Turned on again while the attribute still says hidden.
    ctx.graph.setNodeAttribute(b1, "hidden", true);
    expect(nodeShadows(index, at(NO_FILTERS), a1, 5)).toHaveLength(1);
  });

  it("drops linked and same-entity ends before taking the top, so a node whose best five are linked still shows its sixth (embeddings E1)", () => {
    const nodes = Array.from({ length: 8 }, (_v, i) => node(i + 1));
    const edges: EdgeRow[] = [2, 3, 4, 5, 6].map((j) => [id(100 + j), "supports", id(1), id(j)]);
    const ctx = canvas(nodes, edges, [node(1)]);
    const index = indexPairs([2, 3, 4, 5, 6].map((j) => P(a1, qid("alpha", id(j)), 0.99 - j / 100)).concat([P(a1, qid("alpha", id(7)), 0.9)]));
    expect(nodeShadows(index, ctx, a1, 5)).toEqual([P(a1, qid("alpha", id(7)), 0.9)]);
  });

  it("Selected takes a node's top; All each node's best once, capped and counted", () => {
    const ctx = plain();
    const index = indexPairs([P(a1, b1, 0.95), P(a1, a2, 0.99), P(a1, b2, 0.9), P(a2, b2, 0.91)]);
    expect(nodeShadows(index, ctx, a1, 5).map((p) => p.score)).toEqual([0.95, 0.9]);
    expect(allShadows(index, ctx).pairs.map((p) => p.score)).toEqual([0.95, 0.91]);
    expect(allShadows(index, ctx, null, 1)).toMatchObject({ more: 1 });
  });

  it("gives a merged node its members' shadows, once per far node at the highest score, never to itself", () => {
    const ctx = plain();
    const index = indexPairs([P(a3, a1, 0.88), P(b2, a1, 0.92), P(a3, b2, 0.99)]);
    applyMerge(ctx.graph, ctx.entities);
    const places = mergedPlaces(ctx.graph, ctx.entities);
    const got = nodeShadows(index, ctx, a3, 5, places);
    expect(got).toHaveLength(1);
    expect(got[0].score).toBe(0.92);
    expect(farEnd(got[0], a3, places)).toBe(a1);
  });

  it("draws no merged shadow where another member already has a real edge, the far end merged or not (review B5)", () => {
    // alpha 1 and beta 2 are one entity (k:1); beta 2 supports alpha 3; alpha 1 is similar to alpha 3.
    const ctx = canvas([node(1, ["k:1"]), node(3)], [], [node(2, ["k:1"])]);
    ctx.graph.addEdgeWithKey(qid("beta", id(90)), b2, a3, { kind: "supports", color: "#999", size: 1, hidden: false });
    const index = indexPairs([P(a1, a3, 0.95)]);
    expect(nodeShadows(index, ctx, a1, 5)).toHaveLength(1); // not merged: a1 and a3 are not linked
    applyMerge(ctx.graph, ctx.entities);
    const places = mergedPlaces(ctx.graph, ctx.entities);
    expect(nodeShadows(index, ctx, a1, 5, places)).toEqual([]);
    expect(allShadows(index, ctx, places).pairs).toEqual([]);
    // Both ends merged: alpha 3 and beta 4 share k:2; the edge is beta 2 to beta 4, the similarity alpha 1 to alpha 3.
    const both = canvas([node(1, ["k:1"]), node(3, ["k:2"])], [], [node(2, ["k:1"]), node(4, ["k:2"])], [[id(91), "supports", id(2), id(4)]]);
    applyMerge(both.graph, both.entities);
    const places2 = mergedPlaces(both.graph, both.entities);
    expect(nodeShadows(indexPairs([P(a1, a3, 0.95)]), both, a1, 5, places2)).toEqual([]);
  });

  it("fades from 0.35 at the floor to 0.8 at 1.0, and ranks only when uncalibrated", () => {
    expect(shadowAlpha(0.8462, 0.8462)).toBeCloseTo(0.35);
    expect(shadowAlpha(1, 0.8462)).toBeCloseTo(0.8);
    expect(shadowAlpha(0.7, null)).toBe(0.5);
  });

  it("merges a partial read: pairs between graphs read again are replaced, those touching a held graph stay", () => {
    const old = [P(a1, a2, 0.9), P(a1, b1, 0.8)];
    expect(mergeShadowReads(old, [P(a1, a2, 0.95)], new Set(["alpha"]))).toEqual([P(a1, a2, 0.95), P(a1, b1, 0.8)]);
  });

  it("never uses shadows read over another open set", () => {
    const held = { scope: shadowScope("alpha,vault")!, pairs: [] };
    expect(forScope(held, shadowScope("alpha,vault"))).toBe(held);
    expect(forScope(held, shadowScope("alpha"))).toBeNull();
  });
});

const env = (data: unknown) => ({ umx: 63, command: "", status: "ok" as const, data, warnings: [], errors: [] });
type Call = [string, Record<string, unknown>];
/** A `--rows ids` row (usage 182): the five fields, nothing else. */
const row = (g: string, i: number, og: string, oi: number, score: number) => ({ graph: g, id: id(i), other_graph: og, other_id: id(oi), score });

describe("the server read", () => {
  it("maps lean rows with both ends open to ids and scores, and reads each model key in its own call (embeddings E2, plan 184)", async () => {
    const calls: Call[] = [];
    const read = (async (verb: string, args: Record<string, unknown> = {}) => {
      calls.push([verb, args]);
      if (args.model === "text-embedding-3-small@256") return env({ rows: [row("beta", 1, "gamma", 2, 0.7)], truncated: false, model: "text-embedding-3-small@256", graphs: ["beta", "gamma"], min: { value: 0, calibrated: false }, skipped_model: [] });
      return env({
        rows: [row("alpha", 1, "alpha", 2, 0.9), row("alpha", 1, "vault", 2, 0.97)], truncated: false, model: "embeddinggemma-2@256+p1", graphs: ["alpha"], min: { value: 0.8462, calibrated: true },
        skipped_model: [{ graph: "beta", model: "text-embedding-3-small@256", reason: "another model" }, { graph: "gamma", model: "text-embedding-3-small@256", reason: "another model" }, { graph: "vault", model: "x@1", reason: "another model" }, { graph: "delta", model: "embeddinggemma-2@256+p1", reason: "no vectors yet" }],
      });
    }) as unknown as ReturnType<typeof createUmx>;
    const reply = await loadShadows(read, ["alpha", "beta", "gamma", "delta"]);
    expect(reply.rows).toEqual([["alpha", id(1), "alpha", id(2), 0.9, 0], ["beta", id(1), "gamma", id(2), 0.7, 1]]);
    expect(reply.groups).toEqual([{ model: "embeddinggemma-2", floor: 0.8462, graphs: ["alpha"] }, { model: "text-embedding-3-small", floor: null, graphs: ["beta", "gamma"] }]);
    expect(reply.skipped).toEqual([{ graph: "delta", reason: "no vectors yet", model: "embeddinggemma-2" }]);
    expect(calls.map(([, a]) => a)).toEqual([{ graph: "alpha", with: ["beta", "gamma", "delta"], offset: 0 }, { graph: "beta", with: ["gamma"], offset: 0, model: "text-embedding-3-small@256" }]);
    expect(JSON.stringify(reply)).not.toMatch(/vault/);
  });

  it("stops between pages once the browser has gone, never asking for the next (review B4)", async () => {
    const calls: Call[] = [];
    const ac = new AbortController();
    const read = (async (verb: string, args: Record<string, unknown> = {}) => {
      calls.push([verb, args]);
      ac.abort();
      return env({ rows: [row("alpha", 1, "alpha", 2, 0.9)], truncated: true, model: "m@1", min: {} });
    }) as unknown as ReturnType<typeof createUmx>;
    await expect(loadShadows(read, ["alpha"], ac.signal)).rejects.toMatchObject({ code: "ui_read_cancelled" });
    expect(calls).toHaveLength(1);
  });

  it("reads no open graph with vectors yet as each one not compared, never a failed read (ws009 M2b)", async () => {
    const read = (async () => {
      throw new UmxError("usage_invalid", "umx similarity pairs: no graph in scope has vectors yet (embed one first? or name a model with --model).");
    }) as unknown as ReturnType<typeof createUmx>;
    expect(await loadShadows(read, ["alpha", "gamma"])).toEqual({
      rows: [],
      groups: [],
      skipped: [{ graph: "alpha", reason: "no vectors yet", model: null }, { graph: "gamma", reason: "no vectors yet", model: null }],
    });
    // A read the browser dropped while umx refused stays cancelled (review N2).
    const ac = new AbortController();
    const late = (async () => {
      ac.abort();
      throw new UmxError("usage_invalid", "umx similarity pairs: no graph in scope has vectors yet (embed one first? or name a model with --model).");
    }) as unknown as ReturnType<typeof createUmx>;
    await expect(loadShadows(late, ["alpha"], ac.signal)).rejects.toMatchObject({ code: "ui_read_cancelled" });
    // Any other refusal is still one.
    const other = (async () => {
      throw new UmxError("usage_invalid", "something else");
    }) as unknown as ReturnType<typeof createUmx>;
    await expect(loadShadows(other, ["alpha"])).rejects.toMatchObject({ code: "usage_invalid" });
  });

  it("names the graphs not compared once per reason, in the legend and the panel (ui-expert N2)", () => {
    const none = [{ graph: "crm", reason: "no vectors yet" }, { graph: "ops", reason: "no vectors yet" }, { graph: "hr", reason: "no vectors yet" }];
    expect(skippedLegend(none)).toBe("3 graphs not compared: no vectors yet (crm, ops, hr)");
    expect(skippedPanel(none)).toEqual(["crm, ops, hr have no vectors yet; no shadows to them."]);
    expect(skippedPanel([none[0]])).toEqual(["crm has no vectors yet; no shadows to it."]);
    expect(skippedLegend([])).toBeNull();
  });

  it("says when umx refuses as too large", async () => {
    const read = (async () => {
      throw new UmxError("similarity_pairs_too_large", "too many");
    }) as unknown as ReturnType<typeof createUmx>;
    expect(await loadShadows(read, ["alpha", "beta"])).toMatchObject({ tooLarge: true, rows: [] });
  });
});

/** A daemon the cache tests drive: `status` rows the test edits, and pairs per model key (none: umx's pick). Counts
 * every pairs call by the graphs it named. A pairs reply without its own `compared` reports each compared graph's
 * digest as status gives it when umx copies the vectors (after `duringPairs`, before `afterCopy`). */
function daemon(status: Record<string, unknown>[], pairs: Record<string, Record<string, unknown>>) {
  const asked: string[] = [];
  let statusFails = false;
  const boot = { pid: 4242, started_ms: 1_000, version: { version: "0.0.0", commit: "abc" } };
  const hooks: { duringPairs?: () => void; afterCopy?: () => void } = {};
  const read = (async (verb: string, args: Record<string, unknown> = {}) => {
    if (verb === "embeddings status") {
      if (statusFails) throw new UmxError("daemon_unreachable", "down");
      return env({ graphs: status.map((s) => ({ ...s })) });
    }
    if (verb === "daemon status") return env({ running: true, ...boot });
    asked.push([args.graph, ...((args.with as string[]) ?? [])].join(","));
    // A write that lands while umx reads the pairs: the pairs see it.
    hooks.duringPairs?.();
    const data = pairs[(args.model as string | undefined) ?? ""];
    if (data === undefined) throw new UmxError("usage_invalid", "umx similarity pairs: no graph in scope has vectors yet (embed one first? or name a model with --model).");
    const compared = (data.graphs as string[]).map((g) => ({ graph: g, vectors_digest: status.find((s) => s.graph === g)?.vectors_digest ?? null }));
    const reply = env({ compared, ...data });
    // A write that lands after umx copied the vectors: the reply does not see it.
    hooks.afterCopy?.();
    return reply;
  }) as unknown as ReturnType<typeof createUmx>;
  return { read, asked, boot, hooks, failStatus: () => void (statusFails = true) };
}
const st = (graph: string, digest: string | null, extra: Record<string, unknown> = {}) => ({ graph, visibility: "open", nodes: 3, vectors_done: 3, vectors_digest: digest, ...extra });
const GEM = "embeddinggemma-2@256+p1";
const OAI = "text-embedding-3-small@256";
const one = (rows: object[], graphs: string[], extra: Record<string, unknown> = {}) => ({ rows, truncated: false, model: GEM, graphs, min: { value: 0.8, calibrated: true }, skipped_model: [], ...extra });

describe("the server read keeps answers by vectors_digest (plan 184)", () => {
  it("answers an unchanged digest from what it kept, and reads again after a same-count text edit moves it (review B2)", async () => {
    const status = [st("alpha", "d1"), st("beta", "e1")];
    const pairs = { "": one([row("alpha", 1, "beta", 2, 0.9)], ["alpha", "beta"]) };
    const { read, asked } = daemon(status, pairs);
    const cache = new ShadowCache();
    const first = await loadShadows(read, ["alpha", "beta"], undefined, cache);
    expect(asked).toEqual(["alpha,beta"]);
    expect(await loadShadows(read, ["alpha", "beta"], undefined, cache)).toEqual(first);
    expect(asked).toHaveLength(1);
    // An edit re-embedded: nodes and vectors_done stay 3, the digest moves, the best pair changes.
    status[0].vectors_digest = "d2";
    pairs[""] = one([row("alpha", 1, "beta", 3, 0.93)], ["alpha", "beta"]);
    const after = await loadShadows(read, ["alpha", "beta"], undefined, cache);
    expect(asked).toHaveLength(2);
    expect(after.rows).toEqual([["alpha", id(1), "beta", id(3), 0.93, 0]]);
  });

  it("keeps each model key's call apart: a digest moving in one reads only the calls naming that graph", async () => {
    const status = [st("alpha", "a1"), st("beta", "b1"), st("gamma", "g1")];
    const pairs = {
      "": one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"], { skipped_model: [{ graph: "beta", model: OAI, reason: "another model" }, { graph: "gamma", model: OAI, reason: "another model" }] }),
      [OAI]: one([row("beta", 1, "gamma", 2, 0.7)], ["beta", "gamma"], { model: OAI, min: { value: 0, calibrated: false } }),
    };
    const { read, asked } = daemon(status, pairs);
    const cache = new ShadowCache();
    const first = await loadShadows(read, ["alpha", "beta", "gamma"], undefined, cache);
    expect(asked).toEqual(["alpha,beta,gamma", "beta,gamma"]);
    // alpha is only in the first call (it names every open graph and skips beta and gamma, so it is never kept: plan
    // 189); the OAI call over beta and gamma stands.
    status[0].vectors_digest = "a2";
    expect(await loadShadows(read, ["alpha", "beta", "gamma"], undefined, cache)).toEqual(first);
    expect(asked).toEqual(["alpha,beta,gamma", "beta,gamma", "alpha,beta,gamma"]);
    // gamma moves: both calls name it.
    status[2].vectors_digest = "g2";
    await loadShadows(read, ["alpha", "beta", "gamma"], undefined, cache);
    expect(asked.slice(3)).toEqual(["alpha,beta,gamma", "beta,gamma"]);
  });

  it("never keeps a call naming a graph status gives a null digest, with vectors or without (plan 189)", async () => {
    // No vectors yet: umx refuses the call, the view says so, and asking again asks umx again.
    const none = daemon([st("alpha", null, { vectors_done: 0 }), st("beta", null, { vectors_done: 0 })], {});
    const cache = new ShadowCache();
    const empty = await loadShadows(none.read, ["alpha", "beta"], undefined, cache);
    expect(empty.skipped.map((s) => s.reason)).toEqual(["no vectors yet", "no vectors yet"]);
    await loadShadows(none.read, ["alpha", "beta"], undefined, cache);
    expect([none.asked.length, cache.size]).toEqual([2, 0]);
    // Null on a secured graph the person opened (status never names it), on an open graph with vectors done, with
    // counts unknown, or a graph status does not list: never kept, every read asks umx, even though the pairs reply
    // names a real digest for every graph it compared (umx does for a secured graph the call names; security on plan
    // 189).
    for (const odd of [
      st("vault", null, { visibility: "secured" }),
      st("vault", null),
      st("vault", null, { vectors_done: null }),
      null,
    ]) {
      const status = [st("alpha", "a1"), ...(odd ? [odd] : [])];
      const compared = [{ graph: "alpha", vectors_digest: "a1" }, { graph: "vault", vectors_digest: "v1" }];
      const d = daemon(status, { "": one([row("alpha", 1, "vault", 2, 0.9)], ["alpha", "vault"], { compared }) });
      const c = new ShadowCache();
      const got = await loadShadows(d.read, ["alpha", "vault"], undefined, c);
      expect(got.rows).toEqual([["alpha", id(1), "vault", id(2), 0.9, 0]]);
      await loadShadows(d.read, ["alpha", "vault"], undefined, c);
      expect(d.asked).toHaveLength(2);
      expect(c.size).toBe(0);
    }
  });

  it("reads every time when status fails, and never keeps a read the browser dropped", async () => {
    const d = daemon([st("alpha", "a1")], { "": one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"]) });
    d.failStatus();
    const cache = new ShadowCache();
    await loadShadows(d.read, ["alpha"], undefined, cache);
    await loadShadows(d.read, ["alpha"], undefined, cache);
    expect(d.asked).toHaveLength(2);
    expect(cache.size).toBe(0);
    const ok = daemon([st("alpha", "a1")], {});
    const ac = new AbortController();
    const late = (async (verb: string, args: Record<string, unknown> = {}) => {
      if (verb !== "similarity pairs") return ok.read(verb, args);
      ac.abort();
      return env(one([row("alpha", 1, "alpha", 2, 0.5)], ["alpha"]));
    }) as unknown as ReturnType<typeof createUmx>;
    await expect(loadShadows(late, ["alpha"], ac.signal, cache)).rejects.toMatchObject({ code: "ui_read_cancelled" });
    expect(cache.size).toBe(0);
  });

  it("under Hold reads only the graphs with nothing held, apart from the full set, and Apply with nothing moved reads nothing", async () => {
    const status = [st("alpha", "a1"), st("beta", "b1")];
    const pairs = { "": one([row("alpha", 1, "beta", 1, 0.8), row("alpha", 1, "alpha", 2, 0.95)], ["alpha", "beta"]) };
    const d = daemon(status, pairs);
    const cache = new ShadowCache();
    const fetcher = (graphs: string[], signal: AbortSignal) => loadShadows(d.read, graphs, signal, cache);
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("AB", ["alpha", "beta"], [], true);
    await vi.waitFor(() => expect(reads.state.data).not.toBeNull());
    // beta holds changes: only alpha is read, in a call of its own; the pair touching beta stays drawn.
    pairs[""] = one([row("alpha", 1, "alpha", 2, 0.95)], ["alpha"]);
    reads.update("AB", ["alpha", "beta"], ["beta"], true);
    reads.refresh();
    await vi.waitFor(() => expect(reads.state.loading).toBe(false));
    expect(d.asked).toEqual(["alpha,beta", "alpha"]);
    expect(scores(reads.state)).toEqual([0.95, 0.8]);
    // Applied with no vector moved: the full set's kept answer comes back, umx is not asked.
    reads.update("AB", ["alpha", "beta"], [], true);
    reads.refresh();
    await vi.waitFor(() => expect(reads.state.loading).toBe(false));
    expect(d.asked).toHaveLength(2);
    expect(scores(reads.state)).toEqual([0.95, 0.8]);
    reads.dispose();
  });

  it("keeps pairs a vector landing during the read changed under the digest the reply reports: status A, pairs B (plan 189)", async () => {
    const status = [st("alpha", "dA"), st("beta", "e1")];
    const A = one([row("alpha", 1, "beta", 2, 0.9)], ["alpha", "beta"]);
    const B = one([row("alpha", 1, "beta", 3, 0.93)], ["alpha", "beta"]);
    const pairs: Record<string, Record<string, unknown>> = { "": A };
    const d = daemon(status, pairs);
    const cache = new ShadowCache();
    // The first status sees A; an edit's vector lands before umx copies the vectors, so the pairs and compared are B's.
    d.hooks.duringPairs = () => {
      status[0].vectors_digest = "dB";
      pairs[""] = B;
    };
    const during = await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
    expect(during.rows).toEqual([["alpha", id(1), "beta", id(3), 0.93, 0]]);
    d.hooks.duringPairs = undefined;
    // B stands: answered from what was kept under dB.
    expect(await loadShadows(d.read, ["alpha", "beta"], undefined, cache)).toEqual(during);
    expect(d.asked).toHaveLength(1);
    // Undone: the digest is A's again, and the next Refresh reads A's pairs, never B's.
    status[0].vectors_digest = "dA";
    pairs[""] = A;
    const after = await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
    expect(d.asked).toHaveLength(2);
    expect(after.rows).toEqual([["alpha", id(1), "beta", id(2), 0.9, 0]]);
  });

  it("keeps an edit and its undo inside one read (A, B, A) under B, the digest the reply reports, never under A (F206, plan 189)", async () => {
    const status = [st("alpha", "dA"), st("beta", "e1")];
    const A = one([row("alpha", 1, "beta", 2, 0.9)], ["alpha", "beta"]);
    const B = one([row("alpha", 1, "beta", 3, 0.93)], ["alpha", "beta"]);
    const pairs: Record<string, Record<string, unknown>> = { "": A };
    const d = daemon(status, pairs);
    const cache = new ShadowCache();
    // Status before the read says dA; the edit lands before umx copies the vectors (pairs and compared at dB); the
    // undo lands after the copy, so status after the read says dA again.
    d.hooks.duringPairs = () => {
      status[0].vectors_digest = "dB";
      pairs[""] = B;
    };
    d.hooks.afterCopy = () => {
      status[0].vectors_digest = "dA";
      pairs[""] = A;
    };
    const during = await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
    expect(during.rows).toEqual([["alpha", id(1), "beta", id(3), 0.93, 0]]);
    expect(cache.size).toBe(1);
    d.hooks.duringPairs = d.hooks.afterCopy = undefined;
    // Refresh at dA: nothing is kept under dA, so umx is asked and A's pairs come back.
    const atA = await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
    expect(d.asked).toHaveLength(2);
    expect(atA.rows).toEqual([["alpha", id(1), "beta", id(2), 0.9, 0]]);
    // The edit again (dB): B's pairs were kept under dB, so umx is not asked.
    status[0].vectors_digest = "dB";
    expect(await loadShadows(d.read, ["alpha", "beta"], undefined, cache)).toEqual(during);
    expect(d.asked).toHaveLength(2);
  });

  it("keeps nothing when the reply's compared list lacks a digest for a graph it compared: missing, null, absent, or pages that disagree (plan 189)", async () => {
    const rows = [row("alpha", 1, "beta", 2, 0.9)];
    for (const compared of [
      undefined,
      [{ graph: "alpha", vectors_digest: "a1" }, { graph: "beta", vectors_digest: null }],
      [{ graph: "alpha", vectors_digest: "a1" }],
      [{ graph: "alpha", vectors_digest: "a1" }, { graph: "beta" }],
    ]) {
      const d = daemon([st("alpha", "a1"), st("beta", "b1")], { "": one(rows, ["alpha", "beta"], { compared }) });
      const c = new ShadowCache();
      await loadShadows(d.read, ["alpha", "beta"], undefined, c);
      await loadShadows(d.read, ["alpha", "beta"], undefined, c);
      expect([d.asked.length, c.size]).toEqual([2, 0]);
    }
    // Paged (the fallback): each page compares again, so pages whose compared lists differ keep nothing.
    const pages = [
      { ...one([row("alpha", 1, "beta", 2, 0.9)], ["alpha", "beta"]), truncated: true, compared: [{ graph: "alpha", vectors_digest: "a1" }, { graph: "beta", vectors_digest: "b1" }] },
      { ...one([row("alpha", 2, "beta", 1, 0.85)], ["alpha", "beta"]), compared: [{ graph: "alpha", vectors_digest: "a2" }, { graph: "beta", vectors_digest: "b1" }] },
    ];
    const statusRead = daemon([st("alpha", "a1"), st("beta", "b1")], {}).read;
    let pairsCalls = 0;
    const paged = (async (verb: string, args: Record<string, unknown> = {}) => {
      if (verb !== "similarity pairs") return statusRead(verb, args);
      return env(pages[pairsCalls++ % 2]);
    }) as unknown as ReturnType<typeof createUmx>;
    const c = new ShadowCache();
    expect((await loadShadows(paged, ["alpha", "beta"], undefined, c)).rows).toHaveLength(2);
    expect([pairsCalls, c.size]).toEqual([2, 0]);
  });

  it("never keeps a call with a graph umx skipped, even when status gives that graph the same digest before and after (review, plan 189)", async () => {
    for (const reason of ["no vectors yet", "another model"]) {
      // beta has vectors (digest bA) at both status reads, but none umx could compare during the pairs read: skipped,
      // so compared names only alpha.
      const status = [st("alpha", "a1"), st("beta", "bA")];
      const pairs: Record<string, Record<string, unknown>> = { "": one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"], { skipped_model: [{ graph: "beta", model: GEM, reason }] }) };
      const d = daemon(status, pairs);
      const cache = new ShadowCache();
      await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
      expect(cache.size).toBe(0);
      // Refresh, beta's vectors back in the read: umx is asked again and beta's pair shows.
      pairs[""] = one([row("alpha", 1, "alpha", 2, 0.9), row("alpha", 1, "beta", 2, 0.95)], ["alpha", "beta"]);
      const after = await loadShadows(d.read, ["alpha", "beta"], undefined, cache);
      expect(d.asked.filter((a) => a === "alpha,beta")).toHaveLength(2);
      expect(after.rows).toContainEqual(["alpha", id(1), "beta", id(2), 0.95, 0]);
    }
    // The whole call refused as no vectors yet while status says aA before and after: nothing kept, and a Refresh
    // with alpha's vectors back asks umx again and shows alpha's pair.
    const status = [st("alpha", "aA")];
    const pairs: Record<string, Record<string, unknown>> = {};
    const d = daemon(status, pairs);
    const cache = new ShadowCache();
    expect((await loadShadows(d.read, ["alpha"], undefined, cache)).skipped).toEqual([{ graph: "alpha", reason: "no vectors yet", model: null }]);
    expect(cache.size).toBe(0);
    pairs[""] = one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"]);
    const after = await loadShadows(d.read, ["alpha"], undefined, cache);
    expect([d.asked.length, after.rows, after.skipped]).toEqual([2, [["alpha", id(1), "alpha", id(2), 0.9, 0]], []]);
  });

  it("keeps reads that overlap and finish out of order under the digests their replies report (review B1, plan 189)", async () => {
    const status = [st("alpha", "dA")];
    const boot = { pid: 1, started_ms: 1, version: { commit: "abc" } };
    const answers: ((data: unknown) => void)[] = [];
    let pairsCalls = 0;
    const read = (async (verb: string) => {
      if (verb === "embeddings status") return env({ graphs: status.map((s) => ({ ...s })) });
      if (verb === "daemon status") return env(boot);
      pairsCalls++;
      // apps/ui's TypeScript lib has no Promise.withResolvers.
      return new Promise((resolve) => answers.push((data) => resolve(env(data))));
    }) as unknown as ReturnType<typeof createUmx>;
    const A = one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"], { compared: [{ graph: "alpha", vectors_digest: "dA" }] });
    const B = one([row("alpha", 1, "alpha", 3, 0.93)], ["alpha"], { compared: [{ graph: "alpha", vectors_digest: "dB" }] });
    const cache = new ShadowCache();
    // Read 1 stamps A and waits on umx; the edit lands; read 2 stamps B.
    const r1 = loadShadows(read, ["alpha"], undefined, cache);
    await flush();
    status[0].vectors_digest = "dB";
    const r2 = loadShadows(read, ["alpha"], undefined, cache);
    await flush();
    expect(answers).toHaveLength(2);
    // Read 2 answers first, with B's pairs; read 1 answers last, also with B's (umx copied the vectors after the edit):
    // both are kept under dB, the digest their replies report.
    answers[1](B);
    expect((await r2).rows[0][3]).toBe(id(3));
    answers[0](B);
    expect((await r1).rows[0][3]).toBe(id(3));
    expect(cache.size).toBe(1);
    // B stands: kept. Undone to A: read again, and A's pairs come back, never B's under dA.
    await loadShadows(read, ["alpha"], undefined, cache);
    expect(pairsCalls).toBe(2);
    status[0].vectors_digest = "dA";
    const r3 = loadShadows(read, ["alpha"], undefined, cache);
    await flush();
    expect(pairsCalls).toBe(3);
    answers[2](A);
    expect((await r3).rows[0][3]).toBe(id(2));
  });

  it("drops what another daemon answered: a restart (floor or calibration change) reads again, and an unknown daemon keeps nothing", async () => {
    const d = daemon([st("alpha", "a1")], { "": one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"]) });
    const cache = new ShadowCache();
    await loadShadows(d.read, ["alpha"], undefined, cache);
    await loadShadows(d.read, ["alpha"], undefined, cache);
    expect(d.asked).toHaveLength(1);
    // Restarted with a new floor: same digests, another daemon.
    const pairs = one([row("alpha", 1, "alpha", 2, 0.9)], ["alpha"], { min: { value: 0.85, calibrated: true } });
    const fresh = daemon([st("alpha", "a1")], { "": pairs });
    Object.assign(fresh.boot, { pid: 4343, started_ms: 2_000 });
    const after = await loadShadows(fresh.read, ["alpha"], undefined, cache);
    expect(fresh.asked).toHaveLength(1);
    expect(after.groups[0].floor).toBe(0.85);
    // A daemon status without its start time: nothing is kept.
    const odd = daemon([st("alpha", "a1")], { "": pairs });
    (odd.boot as Record<string, unknown>).started_ms = undefined;
    const c = new ShadowCache();
    await loadShadows(odd.read, ["alpha"], undefined, c);
    await loadShadows(odd.read, ["alpha"], undefined, c);
    expect([odd.asked.length, c.size]).toEqual([2, 0]);
  });

  it("keeps a call's first-page facts only, never the page and its rows (review N2)", async () => {
    const page = one([{ ...row("alpha", 1, "alpha", 2, 0.9), title: "PAGE-ROW-TEXT" }], ["alpha"], { nodes: 2000, extra: "PAGE-EXTRA" });
    const d = daemon([st("alpha", "a1")], { "": page });
    const cache = new ShadowCache();
    await loadShadows(d.read, ["alpha"], undefined, cache);
    // What the cache holds is private; the test looks at it directly, as nothing else shows it.
    const held = cache as unknown as { entries: Map<string, unknown> };
    const kept = [...held.entries.values()];
    expect(kept).toEqual([{ meta: { model: GEM, floor: 0.8, graphs: ["alpha"], skipped: [] }, rows: [["alpha", id(1), "alpha", id(2), 0.9]], cut: false }]);
  });

  it("keeps at most CACHE_ENTRIES answers and CACHE_ROWS rows, oldest dropped first, the newest always", () => {
    const c = new ShadowCache();
    const got = (n: number) => ({ meta: null, rows: Array.from({ length: n }, () => ["a", "b", "a", "c", 0.9] as [string, string, string, string, number]), cut: false });
    for (let i = 0; i < CACHE_ENTRIES + 2; i++) c.put(`k${i}`, got(1));
    expect(c.size).toBe(CACHE_ENTRIES);
    expect(c.get("k0")).toBeUndefined();
    expect(c.get("k2")).toBeDefined();
    c.put("big", got(CACHE_ROWS));
    expect(c.size).toBe(1);
    expect(c.get("big")).toBeDefined();
    c.put("huge", got(CACHE_ROWS + 1));
    expect([c.size, c.get("huge") !== undefined]).toEqual([1, true]);
  });
});

/** Where a graph's vectors are, as umx compares them: its model (graph.json's key, else its embedder's; null: none)
 * and whether any node has a vector there yet. Status's `key` is set apart, in the status row: only graph.json's. */
type Space = { model: string | null; vectors: boolean };
const sp = (model: string | null, vectors = true): Space => ({ model, vectors });
/** A umx that picks and compares like `similarity pairs` (plan 193, pairs.rs): `--model`, or else the first named
 * graph with vectors decides; only named graphs with vectors in that model are compared (the others skipped as another
 * model or no vectors yet); rows only between compared graphs; each compared graph's digest as status gives it (a
 * secured graph status gives none: umx still reports one when the call names it). */
function spaces(status: Record<string, unknown>[], truth: Record<string, Space>, rows: ReturnType<typeof row>[]) {
  const calls: Record<string, unknown>[] = [];
  const boot = { pid: 4242, started_ms: 1_000, version: { version: "0.0.0", commit: "abc" } };
  const read = (async (verb: string, args: Record<string, unknown> = {}) => {
    if (verb === "embeddings status") return env({ graphs: status.map((s) => ({ ...s })) });
    if (verb === "daemon status") return env({ running: true, ...boot });
    calls.push(args);
    const named = [args.graph as string, ...((args.with as string[]) ?? [])];
    const model = (args.model as string | undefined) ?? named.map((g) => truth[g]).find((s) => s.model && s.vectors)?.model;
    if (!model) throw new UmxError("usage_invalid", "umx similarity pairs: no graph in scope has vectors yet (embed one first? or name a model with --model).");
    const compares = (g: string) => truth[g].model === model && truth[g].vectors;
    const graphs = named.filter(compares);
    return env({
      rows: rows.filter((r) => graphs.includes(r.graph) && graphs.includes(r.other_graph)),
      truncated: false,
      model,
      graphs,
      min: { value: 0.8, calibrated: true },
      compared: graphs.map((g) => ({ graph: g, vectors_digest: status.find((s) => s.graph === g)?.vectors_digest ?? `${g}-read` })),
      skipped_model: named
        .filter((g) => !compares(g))
        .map((g) => ({ graph: g, model: truth[g].model, reason: truth[g].model && truth[g].vectors ? "another model" : "no vectors yet" })),
    });
  }) as unknown as ReturnType<typeof createUmx>;
  return { read, calls };
}
/** A reply as a set: rows by their group's model, groups and skipped sorted (umx's order may differ). */
const asSet = (r: ShadowReply) => ({
  rows: r.rows.map(([a, ia, b, ib, s, g]) => JSON.stringify([a, ia, b, ib, s, r.groups[g].model])).sort(),
  groups: r.groups.map((g) => JSON.stringify(g)).sort(),
  skipped: r.skipped.map((s) => JSON.stringify(s)).sort(),
});
const X = "x-embed@8";

describe("the server read's first calls are one per model key (plan 193)", () => {
  const rows = [
    row("alpha", 1, "beta", 2, 0.9),
    row("beta", 2, "alpha", 1, 0.9),
    row("alpha", 1, "vault", 3, 0.88),
    row("gamma", 1, "delta", 2, 0.7),
    row("fresh", 1, "beta", 2, 0.91),
    row("beta", 1, "gamma", 2, 0.8),
  ];

  it("reads two keys in a call each, naming its key, keeps both, and asks umx nothing the second time", async () => {
    const status = [st("alpha", "a1", { key: GEM }), st("beta", "b1", { key: GEM }), st("gamma", "g1", { key: OAI }), st("delta", "d1", { key: OAI })];
    const truth = { alpha: sp(GEM), beta: sp(GEM), gamma: sp(OAI), delta: sp(OAI) };
    const u = spaces(status, truth, rows);
    const cache = new ShadowCache();
    const open = ["alpha", "gamma", "beta", "delta"];
    const first = await loadShadows(u.read, open, undefined, cache);
    expect(u.calls).toEqual([{ graph: "alpha", with: ["beta"], offset: 0, model: GEM }, { graph: "gamma", with: ["delta"], offset: 0, model: OAI }]);
    expect(cache.size).toBe(2);
    expect(await loadShadows(u.read, open, undefined, cache)).toEqual(first);
    expect(u.calls).toHaveLength(2);
    // The same groups, rows and skipped as one call with no model and its follow-up (no cache: today's calls).
    expect(asSet(first)).toEqual(asSet(await loadShadows(spaces(status, truth, rows).read, open)));
  });

  it("compares a secured graph with its key's graphs in a call never kept, and keeps the other key's call", async () => {
    // vault is secured: status lists its key but no digest.
    const status = [st("alpha", "a1", { key: GEM }), st("vault", null, { key: GEM, visibility: "secured" }), st("gamma", "g1", { key: OAI }), st("delta", "d1", { key: OAI })];
    const truth = { alpha: sp(GEM), vault: sp(GEM), gamma: sp(OAI), delta: sp(OAI) };
    const open = ["alpha", "gamma", "vault", "delta"];
    const u = spaces(status, truth, rows);
    const cache = new ShadowCache();
    const first = await loadShadows(u.read, open, undefined, cache);
    expect(u.calls).toEqual([{ graph: "alpha", with: ["vault"], offset: 0, model: GEM }, { graph: "gamma", with: ["delta"], offset: 0, model: OAI }]);
    expect(first.rows).toContainEqual(["alpha", id(1), "vault", id(3), 0.88, 0]);
    expect(cache.size).toBe(1);
    await loadShadows(u.read, open, undefined, cache);
    expect(u.calls.slice(2)).toEqual([{ graph: "alpha", with: ["vault"], offset: 0, model: GEM }]);
    expect(asSet(first)).toEqual(asSet(await loadShadows(spaces(status, truth, rows).read, open)));
  });

  it("makes today's calls when status gives an open graph no key, though it has vectors and a digest (review B1, B2)", async () => {
    // fresh records no key, but its embedder's model is OAI and it has vectors there, like beta.
    const truth = { fresh: sp(OAI), alpha: sp(GEM), beta: sp(OAI) };
    const open = ["fresh", "alpha", "beta"];
    for (const fresh of [st("fresh", null), st("fresh", "f1"), null]) {
      const status = [...(fresh ? [fresh] : []), st("alpha", "a1", { key: GEM }), st("beta", "b1", { key: OAI })];
      const u = spaces(status, truth, rows);
      const cache = new ShadowCache();
      const first = await loadShadows(u.read, open, undefined, cache);
      expect(u.calls).toEqual([{ graph: "fresh", with: ["alpha", "beta"], offset: 0 }, { graph: "alpha", with: [], offset: 0, model: GEM }]);
      expect(first.rows).toContainEqual(["fresh", id(1), "beta", id(2), 0.91, 0]);
      expect(asSet(first)).toEqual(asSet(await loadShadows(spaces(status, truth, rows).read, open)));
      // The first call skipped alpha, so it is not kept: the next read makes it again, and only it.
      await loadShadows(u.read, open, undefined, cache);
      expect(u.calls.slice(2)).toEqual([{ graph: "fresh", with: ["alpha", "beta"], offset: 0 }]);
    }
  });

  it("adds no group for a key whose graphs have no vectors yet, only what it skipped (review B3)", async () => {
    const status = [st("alpha", null, { key: GEM, vectors_done: 0 }), st("beta", "b1", { key: OAI }), st("gamma", "g1", { key: OAI })];
    const truth = { alpha: sp(GEM, false), beta: sp(OAI), gamma: sp(OAI) };
    const open = ["alpha", "beta", "gamma"];
    const u = spaces(status, truth, rows);
    const got = await loadShadows(u.read, open, undefined, new ShadowCache());
    expect(u.calls.map((c) => c.model)).toEqual([GEM, OAI]);
    expect(got.groups).toEqual([{ model: "text-embedding-3-small", floor: 0.8, graphs: ["beta", "gamma"] }]);
    expect(got.rows).toEqual([["beta", id(1), "gamma", id(2), 0.8, 0]]);
    expect(got.skipped).toEqual([{ graph: "alpha", reason: "no vectors yet", model: "embeddinggemma-2" }]);
    expect(asSet(got)).toEqual(asSet(await loadShadows(spaces(status, truth, rows).read, open)));
  });

  it("follows a graph umx puts in a model status did not give it, keeping nothing for the call that skipped it", async () => {
    // Status says gamma is GEM; by the read its vectors are in X.
    const status = [st("alpha", "a1", { key: GEM }), st("beta", "b1", { key: GEM }), st("gamma", "g1", { key: GEM }), st("delta", "d1", { key: OAI })];
    const truth = { alpha: sp(GEM), beta: sp(GEM), gamma: sp(X), delta: sp(OAI) };
    const open = ["alpha", "beta", "gamma", "delta"];
    const u = spaces(status, truth, rows);
    const cache = new ShadowCache();
    const first = await loadShadows(u.read, open, undefined, cache);
    expect(u.calls).toEqual([
      { graph: "alpha", with: ["beta", "gamma"], offset: 0, model: GEM },
      { graph: "delta", with: [], offset: 0, model: OAI },
      { graph: "gamma", with: [], offset: 0, model: X },
    ]);
    expect(asSet(first)).toEqual(asSet(await loadShadows(spaces(status, truth, rows).read, open)));
    await loadShadows(u.read, open, undefined, cache);
    expect(u.calls.slice(3).map((c) => c.model)).toEqual([GEM]);
  });

  it("puts a graph whose key moved into a model still to be asked in that call, and one moved into a model already asked in skipped (the project manager on plan 193)", async () => {
    const status = [st("beta", "b1", { key: OAI }), st("alpha", "a1", { key: GEM }), st("gamma", "g1", { key: GEM }), st("delta", "d1", { key: X })];
    // gamma moved to X, still queued when GEM's call skips it: X's call takes it.
    const queued = spaces(status, { beta: sp(OAI), alpha: sp(GEM), gamma: sp(X), delta: sp(X) }, rows);
    const open = ["beta", "alpha", "gamma", "delta"];
    const merged = await loadShadows(queued.read, open, undefined, new ShadowCache());
    expect(queued.calls.map((c) => [c.graph, c.with, c.model])).toEqual([["beta", [], OAI], ["alpha", ["gamma"], GEM], ["delta", ["gamma"], X]]);
    expect(merged.rows).toContainEqual(["gamma", id(1), "delta", id(2), 0.7, 2]);
    expect(merged.skipped).toEqual([]);
    // gamma moved to OAI, already asked: not asked again, said so, and the GEM call is not kept.
    const asked = spaces(status, { beta: sp(OAI), alpha: sp(GEM), gamma: sp(OAI), delta: sp(X) }, rows);
    const cache = new ShadowCache();
    const got = await loadShadows(asked.read, open, undefined, cache);
    expect(asked.calls.map((c) => c.model)).toEqual([OAI, GEM, X]);
    expect(got.skipped).toEqual([{ graph: "gamma", reason: "another model", model: "text-embedding-3-small" }]);
    expect(cache.size).toBe(2);
    await loadShadows(asked.read, open, undefined, cache);
    expect(asked.calls.slice(3).map((c) => c.model)).toEqual([GEM]);
  });

  it("makes today's calls with one key, and with no cache reads no status", async () => {
    const u = spaces([st("alpha", "a1", { key: GEM }), st("beta", "b1", { key: GEM })], { alpha: sp(GEM), beta: sp(GEM) }, rows);
    await loadShadows(u.read, ["alpha", "beta"], undefined, new ShadowCache());
    expect(u.calls).toEqual([{ graph: "alpha", with: ["beta"], offset: 0 }]);
    const verbs: string[] = [];
    const two = spaces([st("alpha", "a1", { key: GEM }), st("gamma", "g1", { key: OAI })], { alpha: sp(GEM), gamma: sp(OAI) }, rows);
    const spy = (async (verb: string, args?: Record<string, unknown>) => {
      verbs.push(verb);
      return two.read(verb, args);
    }) as unknown as ReturnType<typeof createUmx>;
    await loadShadows(spy, ["alpha", "gamma"]);
    expect(verbs).toEqual(["similarity pairs", "similarity pairs"]);
    expect(two.calls).toEqual([{ graph: "alpha", with: ["gamma"], offset: 0 }, { graph: "gamma", with: [], offset: 0, model: OAI }]);
  });
});

/** A fetch the test answers by hand, in any order. */
function deferred() {
  const asked: { graphs: string[]; signal: AbortSignal; answer: (r: ShadowReply) => void }[] = [];
  const fetcher = (graphs: string[], signal: AbortSignal) => new Promise<ShadowReply>((answer) => asked.push({ graphs, signal, answer }));
  return { asked, fetcher };
}
const reply = (rows: ShadowRow[]): ShadowReply => ({ rows, groups: [{ model: "m", floor: 0.8, graphs: [] }], skipped: [] });
const flush = () => new Promise((r) => setTimeout(r, 0));
const scores = (s: ShadowState) => s.data?.pairs.map((p) => p.score) ?? null;

describe("who owns a shadow read (review B2 to B4)", () => {
  it("lands only the newest read of one scope, whatever order the answers come in", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("A", ["alpha"], [], true);
    reads.refresh();
    expect(asked[0].signal.aborted).toBe(true);
    asked[1].answer(reply([["alpha", id(1), "alpha", id(2), 0.95, 0]]));
    await flush();
    asked[0].answer(reply([["alpha", id(1), "alpha", id(2), 0.5, 0]]));
    await flush();
    expect(scores(reads.state)).toEqual([0.95]);
  });

  it("never lets an old answer for A land after A to B to A", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("A", ["alpha"], [], true);
    reads.update("B", ["beta"], [], true);
    reads.update("A", ["alpha"], [], true);
    expect(asked.map((a) => a.signal.aborted)).toEqual([true, true, false]);
    asked[2].answer(reply([["alpha", id(1), "alpha", id(2), 0.91, 0]]));
    await flush();
    asked[0].answer(reply([["alpha", id(1), "alpha", id(2), 0.1, 0]]));
    asked[1].answer(reply([["beta", id(1), "beta", id(2), 0.2, 0]]));
    await flush();
    expect(reads.state).toMatchObject({ scope: "A" });
    expect(scores(reads.state)).toEqual([0.91]);
  });

  it("reads again on every refresh, with no cache to skip a same-size change (review B2)", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("A", ["alpha"], [], true);
    asked[0].answer(reply([["alpha", id(1), "alpha", id(2), 0.9, 0]]));
    await flush();
    reads.refresh();
    expect(asked).toHaveLength(2);
    asked[1].answer(reply([["alpha", id(1), "alpha", id(3), 0.93, 0]]));
    await flush();
    expect(reads.state.data!.pairs.map((p) => p.b)).toEqual([qid("alpha", id(3))]);
  });

  it("under Hold keeps the applied shadows, reads only graphs with nothing held, and reads the rest once applied (review B3)", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("AB", ["alpha", "beta"], [], true);
    asked[0].answer(reply([["alpha", id(1), "alpha", id(2), 0.9, 0], ["alpha", id(1), "beta", id(1), 0.8, 0]]));
    await flush();
    // Entering Hold: nothing is read, nothing is dropped.
    reads.update("AB", ["alpha", "beta"], [], true);
    expect(asked).toHaveLength(1);
    // beta holds changes; a refresh reads alpha only and keeps the pair into beta as applied.
    reads.update("AB", ["alpha", "beta"], ["beta"], true);
    reads.refresh();
    expect(asked[1].graphs).toEqual(["alpha"]);
    expect(reads.state.waiting).toEqual(["beta"]);
    asked[1].answer(reply([["alpha", id(1), "alpha", id(2), 0.97, 0]]));
    await flush();
    expect(scores(reads.state)).toEqual([0.97, 0.8]);
    // Every graph held: nothing is read at all.
    reads.update("AB", ["alpha", "beta"], ["alpha", "beta"], true);
    reads.refresh();
    expect(asked).toHaveLength(2);
    // beta applied: read again in full.
    reads.update("AB", ["alpha", "beta"], [], true);
    reads.refresh();
    expect(asked[2].graphs).toEqual(["alpha", "beta"]);
  });

  it("stops and drops everything when turned off", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("A", ["alpha"], [], true);
    reads.update("A", ["alpha"], [], false);
    expect(asked[0].signal.aborted).toBe(true);
    asked[0].answer(reply([["alpha", id(1), "alpha", id(2), 0.9, 0]]));
    await flush();
    expect(reads.state.data).toBeNull();
  });
});

describe("round 2 notes", () => {
  it("projects a merged entity onto a member still shown, as the filters stand now, hide and reveal (N1)", () => {
    // alpha 1 and beta 2 are one entity (k:1); beta 2 is newer, so it is the head; alpha 3 is similar to alpha 1.
    const ctx = canvas([[id(1), "risk", "n1", "open", "2026-11-01T00:00:00Z", ["k:1"]], node(3)], [], [[id(2), "risk", "n2", "parked", "2026-11-09T00:00:00Z", ["k:1"]]]);
    const index = indexPairs([P(a1, a3, 0.95)]);
    applyMerge(ctx.graph, ctx.entities);
    const at = (filters: typeof NO_FILTERS) => {
      const shown = shownBy(ctx.graph, shownMask(ctx.graph, { provenance: false, retired: false }, filters));
      return { ctx: { ...ctx, shown }, places: mergedPlaces(ctx.graph, ctx.entities, shown) };
    };
    const far = (filters: typeof NO_FILTERS) => {
      const { ctx: c, places } = at(filters);
      return nodeShadows(index, c, a3, 5, places).map((p) => farEnd(p, a3, places));
    };
    expect(far(NO_FILTERS)).toEqual([b2]);
    // beta 2's status is filtered out; its hidden attribute still says shown.
    const off = { ...NO_FILTERS, statuses: new Set(["parked"]) };
    expect(ctx.graph.getNodeAttribute(b2, "hidden")).toBe(false);
    expect(far(off)).toEqual([a1]);
    expect(far(NO_FILTERS)).toEqual([b2]);
  });

  it("drops a read that was running when a graph began holding changes, and asks again without it (N2)", async () => {
    const { asked, fetcher } = deferred();
    const reads = new ShadowReads(fetcher, () => {});
    reads.update("AB", ["alpha", "beta"], [], true);
    asked[0].answer(reply([["alpha", id(1), "beta", id(1), 0.8, 0]]));
    await flush();
    reads.refresh();
    // Hold, and beta holds a change, while that full read runs.
    reads.update("AB", ["alpha", "beta"], ["beta"], true);
    expect(asked[1].signal.aborted).toBe(true);
    expect(asked[2].graphs).toEqual(["alpha"]);
    asked[1].answer(reply([["alpha", id(1), "beta", id(1), 0.99, 0]]));
    await flush();
    expect(scores(reads.state)).toEqual([0.8]);
    asked[2].answer(reply([]));
    await flush();
    expect(scores(reads.state)).toEqual([0.8]);
  });
});

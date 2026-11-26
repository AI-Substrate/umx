import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { WorkerMessage, WorkerRequest, WorkerStart } from "@/lib/graph/layout.worker";
import { HUB_DEFAULTS, hubSize, type LayoutMode } from "@/lib/graph/hubs";
import { layoutRequest } from "@/components/use-layout";
import { spineLayout, type Place } from "@/lib/graph/spine-layout";
import { clearanceRadius } from "@/lib/graph/clearance";

const replies: WorkerMessage[] = [];
const worker = {
  onmessage: null as ((event: MessageEvent<WorkerRequest>) => void) | null,
  postMessage: (message: WorkerMessage) => replies.push(message),
};
beforeAll(async () => { vi.stubGlobal("self", worker); await import("@/lib/graph/layout.worker"); });
afterAll(() => vi.unstubAllGlobals());

function fixture(n: number, mode: LayoutMode, bands = 10): WorkerStart {
  const rows: [number, number, number][] = [];
  for (let i = 1; i < n; i++) {
    const head = Math.floor(i / Math.ceil(n / bands)) * Math.ceil(n / bands);
    if (i !== head) rows.push([i, head, 1]);
    if (i > head + 1) rows.push([i, i - 1, 0]);
  }
  return {
    kind: "start", id: 17,
    graph: { n, edges: Uint32Array.from(rows.flatMap(([a, b]) => [a, b])), partOf: Uint8Array.from(rows.map((r) => r[2])), similarity: Float32Array.from(rows.map(() => 0.8)) },
    ids: Array.from({ length: n }, (_, i) => `g/${String(i).padStart(6, "0")}`), sizes: new Float32Array(n).fill(4),
    visible: new Uint8Array(n).fill(1), edges: new Uint8Array(rows.length).fill(1),
    resolution: 1, seed: 7, spacing: 1.6, budgetMs: n > 5000 ? 30_000 : 15_000,
    options: { ...HUB_DEFAULTS, mode }, similarities: [],
  };
}
function run(request: WorkerStart) {
  replies.length = 0;
  worker.onmessage!(new MessageEvent("message", { data: request }));
  const result = replies.findLast((m) => m.kind === "done");
  if (!result || result.kind !== "done") throw new Error("worker did not finish");
  return result;
}

describe("layout worker boundary", () => {
  it("lays out hidden nodes once, then filters and resolution changes return no coordinates", () => {
    const request = fixture(30, "normal");
    request.visible[5] = 0;
    request.visible[8] = 0;
    const first = run(request);
    expect(first.ids).toEqual([...request.ids].sort());
    expect(new Set(Array.from({ length: 30 }, (_, i) => `${first.positions[i * 2]},${first.positions[i * 2 + 1]}`)).size).toBe(30);
    const before = { ...request, view: { index: request.graph, ids: request.ids } };
    const visible = request.visible.slice();
    visible[5] = 1;
    visible[3] = 0;
    const edges = request.edges.slice();
    edges[0] = 0;
    for (const change of [{ visible }, { edges }, { resolution: 2 }]) {
      const after = { ...before, ...change };
      const kind = layoutRequest(before, after);
      if (kind !== "cluster") throw new Error("filter and resolution changes must only recluster");
      replies.length = 0;
      worker.onmessage!(new MessageEvent("message", { data: { ...request, ...change, kind } }));
      expect(replies).toHaveLength(1);
      expect(replies[0].kind).toBe("clusters");
      expect(replies[0]).not.toHaveProperty("positions");
    }
    for (const change of [
      { spacing: 3 },
      { options: { ...request.options, mode: "degree" as const } },
      { options: { ...request.options, degree: "incoming" as const } },
      { options: { ...request.options, count: 3 } },
    ]) expect(layoutRequest(before, { ...before, ...change })).toBe("start");
  });

  it("ignores identical shadow re-reads by content and only reclusters changed similarities", () => {
    for (const mode of ["degree", "similarity"] as const) {
      const request = fixture(30, mode);
      const similarities = [
        { a: request.ids[0], b: request.ids[1], score: 0.8 },
        { a: request.ids[0], b: request.ids[2], score: 0.9 },
      ];
      const before = { ...request, view: { index: request.graph, ids: request.ids }, similarities };
      expect(layoutRequest(before, { ...before, similarities: similarities.map((p) => ({ ...p })) })).toBeNull();
      expect(layoutRequest(before, { ...before, similarities: [...similarities].reverse() })).toBeNull();
      const changed = similarities.map((p) => ({ ...p, score: p.score - 0.1 }));
      const kind = layoutRequest(before, { ...before, similarities: changed });
      if (kind !== "cluster") throw new Error("a similarity change must only recluster");
      replies.length = 0;
      worker.onmessage!(new MessageEvent("message", { data: { ...request, similarities: changed, kind } }));
      expect(replies).toHaveLength(1);
      expect(replies[0].kind).toBe("clusters");
      expect(replies[0]).not.toHaveProperty("positions");
    }
  });

  it("publishes hub flags and owners only with full geometry, not when filters or clustering change", () => {
    for (const mode of ["degree", "similarity"] as const) {
      const request = fixture(30, mode);
      const first = run(request);
      expect(first.hubs.centres.length).toBeGreaterThan(0);
      const visible = request.visible.slice();
      visible[request.ids.indexOf(first.hubs.centres[0])] = 0;
      for (const change of [{ visible }, { resolution: 2 }, { graph: { ...request.graph, similarity: undefined } }]) {
        replies.length = 0;
        worker.onmessage!(new MessageEvent("message", { data: { ...request, ...change, kind: "cluster", id: 20 } }));
        expect(replies).toHaveLength(1);
        expect(replies[0]).toMatchObject({ kind: "clusters", id: 20 });
        expect(replies[0]).not.toHaveProperty("hubs");
        expect(replies[0]).not.toHaveProperty("positions");
      }
    }
  });

  it("replaces degree-fallback fans once when usable similarity pairs arrive", () => {
    const request = fixture(30, "similarity");
    request.graph.similarity = undefined;
    const first = run(request);
    expect(first.hubs.fallback).toBe(true);
    const before = { ...request, view: { index: request.graph, ids: request.ids } };
    const similarities = [
      { a: request.ids[29], b: request.ids[27], score: 0.8 },
      { a: request.ids[29], b: request.ids[28], score: 0.9 },
    ];
    const after = { ...before, similarities };
    expect(layoutRequest(before, after, true)).toBe("start");
    const second = run({ ...request, similarities });
    expect(second.hubs.fallback).not.toBe(true);
    expect(second.hubs.centres).toEqual([request.ids[29]]);
    expect(second.hubs.centres).not.toEqual(first.hubs.centres);
    expect(layoutRequest(after, { ...after, similarities: similarities.map((p) => ({ ...p })) }, false)).toBeNull();
    expect(layoutRequest(after, { ...after, similarities: similarities.map((p) => ({ ...p, score: p.score - 0.1 })) }, false)).toBe("cluster");
    for (const unusable of [
      [{ ...similarities[0], score: 0 }],
      [{ ...similarities[0], b: "closed/node" }],
      [{ ...similarities[0], b: similarities[0].a }],
    ]) expect(layoutRequest(before, { ...after, similarities: unusable }, true)).toBe("cluster");
    const visible = request.visible.slice();
    visible[29] = 0;
    expect(layoutRequest(before, { ...after, visible }, true)).toBe("cluster");
  });

  it("returns canonical ids, reproducible coordinates and request generations when modes change", () => {
    const request = fixture(120, "degree");
    const first = run(request);
    run({ ...request, id: 18, options: { ...request.options, mode: "similarity" } });
    const again = run({ ...request, id: 19 });
    expect(again.id).toBe(19);
    expect(again.ids).toEqual(first.ids);
    expect(again.positions).toEqual(first.positions);
    expect(again.hubs).toMatchObject({ centres: first.hubs.centres, owner: first.hubs.owner });
    const capped = run({ ...request, budgetMs: 0 });
    expect(capped.capped).toBe(true);
  });

  for (const n of [747, 1000, 10_000]) {
    it(`finishes all four modes on ${n} synthetic nodes within the existing safety budget`, () => {
      for (const mode of ["normal", "degree", "similarity", "declared"] as const) {
        const request = fixture(n, mode, n === 747 ? 30 : 10);
        if (mode === "declared") {
          const step = Math.ceil(n / (n === 747 ? 30 : 10));
          request.graph.declared = Int8Array.from(request.ids.map((_id, i) => i % step === 0 ? 1 : 0));
          request.graph.membership = request.graph.partOf;
          request.options.count = 1;
        }
        const result = run(request);
        expect(result.capped, `${mode}: ${result.ms}ms`).toBe(false);
        expect(result.ms).toBeLessThan(request.budgetMs);
        expect(result.positions.length).toBe(n * 2);
        expect(result.positions.every(Number.isFinite)).toBe(true);
        if (mode === "declared") {
          expect(result.hubs.centres.length).toBeGreaterThan(request.options.count);
          const heads = new Set(result.hubs.centres);
          const nodes = result.ids.map((key, i) => ({
            key, id: key, x: result.positions[i * 2], y: result.positions[i * 2 + 1],
            clearance: clearanceRadius(heads.has(key) ? hubSize(4) : 4, request.spacing),
          }));
          const owners = new Map(result.hubs.owner);
          const places = new Map<string, Place>(nodes.map((node, i) => [node.id, { band: i >= n * 0.95 ? null : i % 5, by: "field", anchor: i < 5 }]));
          const started = performance.now();
          const projected = spineLayout("g", nodes, places, 1, 1.8, owners, request.spacing, true);
          expect(projected.at.size).toBe(n);
          console.info(`declared-spine-proof nodes=${n} projection_ms=${Math.round(performance.now() - started)}`);
        }
        console.info(`hub-proof nodes=${n} mode=${mode} total_ms=${result.ms} hub_ms=${result.hubs.ms} heap_mb=${Math.round(process.memoryUsage().heapUsed / 1024 / 1024)}`);
      }
    }, 120_000);
  }
});

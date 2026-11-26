import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { WorkerMessage, WorkerRequest, WorkerStart } from "@/lib/graph/layout.worker";
import { HUB_DEFAULTS, type LayoutMode } from "@/lib/graph/hubs";
import { layoutRequest } from "@/components/use-layout";

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
    const before = { ...request, view: { index: request.graph } };
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
      const before = { ...request, view: { index: request.graph }, similarities };
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

  it("refreshes hub metadata after live changes without moving existing nodes", () => {
    const request = fixture(30, "similarity");
    const first = run(request);
    expect(first.hubs.fallback).not.toBe(true);
    replies.length = 0;
    worker.onmessage!(new MessageEvent("message", { data: {
      ...request, kind: "cluster", id: 20, graph: { ...request.graph, similarity: undefined },
    } }));
    expect(replies).toHaveLength(1);
    expect(replies[0]).toMatchObject({ kind: "clusters", id: 20, hubs: { fallback: true } });
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
        const result = run(request);
        expect(result.capped, `${mode}: ${result.ms}ms`).toBe(false);
        expect(result.ms).toBeLessThan(request.budgetMs);
        expect(result.positions.length).toBe(n * 2);
        expect(result.positions.every(Number.isFinite)).toBe(true);
        console.info(`hub-proof nodes=${n} mode=${mode} total_ms=${result.ms} hub_ms=${result.hubs.ms} heap_mb=${Math.round(process.memoryUsage().heapUsed / 1024 / 1024)}`);
      }
    }, 120_000);
  }
});

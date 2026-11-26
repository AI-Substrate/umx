"use client";
import { useEffect, useRef, useState } from "react";
import { type IndexGraph, type Groups } from "@/lib/graph/clusters";
import { applyNudge, nudgeInput } from "@/lib/graph/apply";
import { type ViewGraph } from "@/lib/graph/build";
import { HUB_DEFAULTS, hubSize, type HubOptions, type SimilarityLink } from "@/lib/graph/hubs";
import { clearanceRadius } from "@/lib/graph/clearance";
import type { HubResult, WorkerMessage, WorkerStart, WorkerCluster } from "@/lib/graph/layout.worker";

export type LayoutState = { progress: number; done: boolean; ms?: number; iterations?: number; capped?: boolean; error?: string };
const IDLE: LayoutState = { progress: 0, done: false };
export type Louvain = { groups: Groups; ms: number; resolution: number; ids: readonly string[] };
export type Nudge = { n: number; fresh: readonly string[] };
const NO_PAIRS: readonly SimilarityLink[] = [];
const NO_HUBS: HubResult = { centres: [], owner: [], ms: 0 };

type LayoutInputs = {
  view: { index: IndexGraph; ids: readonly string[] } | null;
  visible: Uint8Array | null;
  edges: Uint8Array | null;
  resolution: number;
  spacing: number;
  options: HubOptions;
  similarities: readonly SimilarityLink[];
};

function sameSimilarities(a: readonly SimilarityLink[], b: readonly SimilarityLink[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  if (a.every((p, i) => p.a === b[i].a && p.b === b[i].b && p.score === b[i].score)) return true;
  const key = (p: SimilarityLink) => JSON.stringify([p.a, p.b, p.score]);
  const counts = new Map<string, number>();
  for (const p of a) counts.set(key(p), (counts.get(key(p)) ?? 0) + 1);
  for (const p of b) {
    const k = key(p), count = counts.get(k);
    if (!count) return false;
    if (count === 1) counts.delete(k);
    else counts.set(k, count - 1);
  }
  return true;
}

/** Regrouping keeps its geometry; controls or the first usable similarity data can replace a full fan layout. */
export function layoutRequest(before: LayoutInputs, after: LayoutInputs, similarityFallback = false): "start" | "cluster" | null {
  if (before.spacing !== after.spacing || before.options.mode !== after.options.mode || before.options.degree !== after.options.degree || before.options.count !== after.options.count) return "start";
  const pairsChanged = !sameSimilarities(before.similarities, after.similarities);
  if (similarityFallback && after.options.mode === "similarity" && pairsChanged) {
    const shown = new Set(after.view?.ids.filter((_id, i) => after.visible?.[i]));
    if (after.similarities.some((p) => p.a !== p.b && Number.isFinite(p.score) && p.score > 0 && shown.has(p.a) && shown.has(p.b))) return "start";
  }
  const same = (a: Uint8Array | null, b: Uint8Array | null) => a === b || (a !== null && b !== null && a.length === b.length && a.every((v, i) => v === b[i]));
  if (before.view?.index !== after.view?.index || before.resolution !== after.resolution || !same(before.visible, after.visible) || !same(before.edges, after.edges) || pairsChanged) return "cluster";
  return null;
}

/** One worker per open graph set; one running request and only the newest pending request. Results carry their
 * generation and ids, so an obsolete mode or a live renumbering can never put coordinates on the wrong nodes. */
export function useLayout(
  view: { graph: ViewGraph; index: IndexGraph; ids: readonly string[]; seed: number; nudge?: Nudge | null; keep?: ReadonlyMap<string, { x: number; y: number }> } | null,
  visible: Uint8Array | null,
  edges: Uint8Array | null,
  resolution: number,
  spacing: number,
  options: HubOptions = HUB_DEFAULTS,
  similarities: readonly SimilarityLink[] = NO_PAIRS,
) {
  const [layoutFor, setLayout] = useState<{ graph: ViewGraph; value: LayoutState } | null>(null);
  const [nudged, setNudged] = useState(0);
  const [louvainFor, setLouvain] = useState<{ graph: ViewGraph; value: Louvain } | null>(null);
  const [hubsFor, setHubs] = useState<{ graph: ViewGraph; value: HubResult } | null>(null);
  const args = useRef({ view, visible, edges, resolution, spacing, options, similarities });
  useEffect(() => { args.current = { view, visible, edges, resolution, spacing, options, similarities }; });
  const live = useRef<{
    worker: Worker; busy: boolean; next: number; latest: number; runningFull: boolean; similarityFallback: boolean;
    pending: WorkerStart | WorkerCluster | null;
    sent: typeof args.current;
    asked: Map<number, { resolution: number; ids: readonly string[] }>;
    nudges: Map<number, { ids: string[]; fixed: Uint8Array; generation: number }>;
    request: (full: boolean, keep?: boolean) => void;
  } | null>(null);
  const graph = view?.graph ?? null;

  useEffect(() => {
    if (!graph || !args.current.view || !args.current.visible || !args.current.edges) return;
    const worker = new Worker(new URL("../lib/graph/layout.worker.ts", import.meta.url), { type: "module" });
    const state: NonNullable<typeof live.current> = {
      worker, busy: false, next: 0, latest: 0, runningFull: false, similarityFallback: false, pending: null, sent: args.current,
      asked: new Map(), nudges: new Map(),
      request: (full, keep = false) => {
        const a = args.current, v = a.view;
        if (!v || !a.visible || !a.edges) return;
        full ||= state.runningFull || state.pending?.kind === "start";
        const id = ++state.next;
        state.latest = id;
        const sizes = new Float32Array(v.index.n);
        graph.forEachNode((_k, n) => { sizes[n.index] = n.size; });
        const request: WorkerStart | WorkerCluster = {
          kind: full ? "start" : "cluster", id, graph: v.index, ids: v.ids, sizes,
          visible: a.visible, edges: a.edges, resolution: a.resolution, seed: v.seed, spacing: a.spacing,
          budgetMs: v.index.n > 5000 ? 30_000 : 15_000, options: a.options, similarities: a.similarities,
        };
        if (keep && v.keep?.size) {
          const positions = new Float32Array(v.index.n * 2), fixed = new Uint8Array(v.index.n);
          v.ids.forEach((key, i) => {
            const p = v.keep!.get(key);
            if (p) [positions[i * 2], positions[i * 2 + 1], fixed[i]] = [p.x, p.y, 1];
          });
          request.keep = { positions, fixed };
        }
        state.sent = a;
        state.asked.set(id, { resolution: a.resolution, ids: v.ids });
        if (full) setLayout({ graph, value: IDLE });
        // A live-data cluster request must not replace a pending full layout.
        if (state.pending) state.asked.delete(state.pending.id);
        if (state.busy) state.pending = request;
        else send(request);
      },
    };
    live.current = state;
    worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
      const m = event.data;
      if (m.kind === "nudged") {
        const asked = state.nudges.get(m.id);
        state.nudges.delete(m.id);
        if (asked?.generation === state.latest) {
          applyNudge(graph, asked.ids, asked.fixed, m.positions);
          asked.ids.forEach((id, i) => { if (!asked.fixed[i] && graph.hasNode(id)) graph.setNodeAttribute(id, "clearance", clearanceRadius(graph.getNodeAttribute(id, "size"), args.current.spacing)); });
          setNudged((n) => n + 1);
        }
        return;
      }
      if (m.kind === "clusters") {
        const asked = state.asked.get(m.id);
        if (m.id === state.latest && asked) {
          setLouvain({ graph, value: { groups: m.groups, ms: m.ms, resolution: asked.resolution, ids: asked.ids } });
        }
        // Full requests send coordinates after their clusters; cluster-only requests finish here.
        if (state.runningFull) return;
      } else if (m.id === state.latest) {
        const centres = m.kind === "done" ? new Set(m.hubs.centres) : null;
        graph.updateEachNodeAttributes((id, a) => {
          if (centres) a.hub = centres.has(id);
          const i = positionIndex.get(id);
          if (i !== undefined) {
            a.clearance = clearanceRadius(a.hub ? hubSize(a.size) : a.size, state.sent.spacing);
            if (a.homeX !== undefined) [a.homeX, a.homeY] = [m.positions[i * 2], m.positions[i * 2 + 1]];
            else [a.x, a.y] = [m.positions[i * 2], m.positions[i * 2 + 1]];
          }
          return a;
        }, { attributes: ["x", "y"] });
        if (m.kind === "done") {
          state.similarityFallback = m.hubs.fallback === true;
          setLayout({ graph, value: { progress: 1, done: true, ms: m.ms, iterations: m.iterations, capped: m.capped } });
          setHubs({ graph, value: m.hubs });
        } else setLayout({ graph, value: { progress: m.progress, done: false } });
      }
      if (m.kind === "tick") return;
      state.asked.delete(m.id);
      state.busy = false;
      state.runningFull = false;
      if (state.pending) {
        const next = state.pending;
        state.pending = null;
        send(next);
      }
    };
    let positionIndex = new Map<string, number>();
    const send = (request: WorkerStart | WorkerCluster) => {
      state.busy = true;
      state.runningFull = request.kind === "start";
      positionIndex = new Map([...request.ids].sort().map((id, i) => [id, i]));
      worker.postMessage(request);
    };
    worker.onerror = (error) => {
      setLayout({ graph, value: { progress: 0, done: true, error: error.message || "Layout worker failed" } });
      state.busy = false;
    };
    state.request(true, true);
    return () => { worker.terminate(); if (live.current === state) live.current = null; };
  }, [graph]);

  useEffect(() => {
    const state = live.current;
    if (!state || !view || !visible || !edges) return;
    const request = layoutRequest(state.sent, args.current, state.similarityFallback);
    if (request) state.request(request === "start");
    else state.sent = args.current;
  }, [view, visible, edges, resolution, spacing, options, similarities]);

  const nudge = view?.nudge;
  useEffect(() => {
    const state = live.current;
    if (!state || !view || !nudge?.fresh.length) return;
    const made = nudgeInput(view.graph, nudge.fresh);
    if (!made) return;
    state.nudges.set(nudge.n, { ids: made.ids, fixed: made.input.fixed, generation: state.latest });
    state.worker.postMessage({ kind: "nudge", id: nudge.n, ...made.input, spacing: args.current.spacing });
  }, [view, nudge]);

  return {
    layout: layoutFor?.graph === graph ? layoutFor.value : IDLE,
    louvain: louvainFor?.graph === graph ? louvainFor.value : null,
    hubs: hubsFor?.graph === graph ? hubsFor.value : NO_HUBS,
    nudged,
  };
}

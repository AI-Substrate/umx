"use client";
import { useEffect, useRef, useState } from "react";
import { type IndexGraph, type Groups } from "@/lib/graph/clusters";
import { applyNudge, nudgeInput } from "@/lib/graph/apply";
import { type ViewGraph } from "@/lib/graph/build";
import type { WorkerCluster, WorkerMessage, WorkerRelayout, WorkerStart } from "@/lib/graph/layout.worker";

export type LayoutState = { progress: number; done: boolean; ms?: number; iterations?: number; capped?: boolean };
const IDLE: LayoutState = { progress: 0, done: false };
/** `ids`: the node ids the groups' indices refer to (a live change renumbers nodes; lib/graph/apply remapGroups). */
export type Louvain = { groups: Groups; ms: number; resolution: number; ids: readonly string[] };

/** A live change's new nodes, to settle with a nudge; `n` changes on every change. */
export type Nudge = { n: number; fresh: readonly string[] };

/** Runs the layout worker for one graph: lays it out once (seeded by its clusters, at `spacing`), and reruns Louvain
 * whenever the visible set, the resolution or the graph's data (a live change) changes, sending only the latest request
 * when the worker is busy. A live change never lays the graph out again: its new nodes get a short nudge. A new
 * spacing lays it out again from where its nodes are (plan 198), the newest spacing only once a run ends. */
export function useLayout(
  view: { graph: ViewGraph; index: IndexGraph; ids: readonly string[]; seed: number; nudge?: Nudge | null; keep?: ReadonlyMap<string, { x: number; y: number }> } | null,
  visible: Uint8Array | null,
  edges: Uint8Array | null,
  resolution: number,
  spacing: number,
) {
  // Each result is kept with the graph it is for, so a new graph starts clean without a reset in the effect.
  const [layoutFor, setLayout] = useState<{ graph: ViewGraph; value: LayoutState } | null>(null);
  // Nudges applied, so merged entities (ws009 M2) take their members' settled places (review B6).
  const [nudged, setNudged] = useState(0);
  const [louvainFor, setLouvain] = useState<{ graph: ViewGraph; value: Louvain } | null>(null);
  const live = useRef<{
    worker: Worker;
    latest: number;
    busy: boolean;
    pending: WorkerCluster | null;
    sent: { visible: Uint8Array; edges: Uint8Array; resolution: number; index: IndexGraph };
    asked: Map<number, { resolution: number; ids: readonly string[] }>;
    nudges: Map<number, { ids: string[]; fixed: Uint8Array }>;
    /** A layout (the first, or a relayout) is running; `spacing` is the one it runs at, `wanted` the newest asked. */
    laying: boolean;
    spacing: number;
    wanted: number;
    relayout: (spacing: number) => void;
  } | null>(null);
  // The worker starts with the settings and data of the render that made the view; effects run in order, so this
  // is current.
  const args = useRef({ visible, edges, resolution, spacing, view });
  useEffect(() => {
    args.current = { visible, edges, resolution, spacing, view };
  });
  const graph = view?.graph ?? null;

  useEffect(() => {
    const v = args.current.view;
    if (!graph || !v || !args.current.visible || !args.current.edges) return;
    const { index, seed, ids } = v;
    const worker = new Worker(new URL("../lib/graph/layout.worker.ts", import.meta.url), { type: "module" });
    const budgetMs = (n: number) => (n > 5000 ? 30_000 : 15_000);
    const sizesOf = (n: number) => {
      const sizes = new Float32Array(n);
      graph.forEachNode((_id, a) => {
        sizes[a.index] = a.size;
      });
      return sizes;
    };
    const state = {
      worker,
      latest: 0,
      busy: true,
      pending: null as WorkerCluster | null,
      sent: { visible: args.current.visible!, edges: args.current.edges!, resolution: args.current.resolution, index },
      asked: new Map([[0, { resolution: args.current.resolution, ids }]]),
      nudges: new Map<number, { ids: string[]; fixed: Uint8Array }>(),
      laying: true,
      spacing: args.current.spacing,
      wanted: args.current.spacing,
      // Warm, from the positions on screen (a merged member's own, not its entity's), over the graph as it is now.
      relayout: (spacing: number) => {
        const now = args.current.view?.index ?? index;
        const positions = new Float32Array(now.n * 2);
        graph.forEachNode((_id, a) => {
          [positions[a.index * 2], positions[a.index * 2 + 1]] = [a.homeX ?? a.x, a.homeY ?? a.y];
        });
        const request: WorkerRelayout = { kind: "relayout", graph: now, positions, sizes: sizesOf(now.n), spacing, budgetMs: budgetMs(now.n) };
        state.laying = true;
        state.spacing = spacing;
        setLayout({ graph, value: { progress: 0, done: false } });
        worker.postMessage(request, [positions.buffer]);
      },
    };
    live.current = state;
    const apply = (p: Float32Array) =>
      graph.updateEachNodeAttributes(
        (_node, a) => {
          // A merged member (ws009 M2) is drawn at its entity's centroid: the layout moves where it is laid out.
          if (a.homeX !== undefined) [a.homeX, a.homeY] = [p[a.index * 2], p[a.index * 2 + 1]];
          else [a.x, a.y] = [p[a.index * 2], p[a.index * 2 + 1]];
          return a;
        },
        { attributes: ["x", "y"] },
      );
    worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
      const m = event.data;
      if (m.kind === "clusters") {
        const asked = state.asked.get(m.id);
        state.asked.delete(m.id);
        if (m.id === state.latest && asked) {
          setLouvain({ graph, value: { groups: m.groups, ms: m.ms, resolution: asked.resolution, ids: asked.ids } });
        }
        state.busy = false;
        if (state.pending) {
          worker.postMessage(state.pending);
          state.pending = null;
          state.busy = true;
        }
        return;
      }
      if (m.kind === "nudged") {
        const asked = state.nudges.get(m.id);
        state.nudges.delete(m.id);
        if (asked) {
          applyNudge(graph, asked.ids, asked.fixed, m.positions);
          setNudged((k) => k + 1);
        }
        return;
      }
      apply(m.positions);
      if (m.kind === "done") {
        setLayout({ graph, value: { progress: 1, done: true, ms: m.ms, iterations: m.iterations, capped: m.capped } });
        state.laying = false;
        if (state.wanted !== state.spacing) state.relayout(state.wanted);
      } else {
        setLayout({ graph, value: { progress: m.progress, done: false } });
      }
    };
    const n = graph.order;
    const start: WorkerStart = {
      kind: "start",
      graph: index,
      sizes: sizesOf(n),
      visible: state.sent.visible,
      edges: state.sent.edges,
      resolution: state.sent.resolution,
      seed,
      spacing: state.spacing,
      // A safety cap only: the layout stops on its iteration count (ui-expert N3).
      budgetMs: budgetMs(n),
    };
    // An agent opened more graphs (ws009 M1b1): the nodes already on screen keep their positions.
    if (v.keep && v.keep.size > 0) {
      const positions = new Float32Array(n * 2);
      const fixed = new Uint8Array(n);
      ids.forEach((key, i) => {
        const at = v.keep!.get(key);
        if (at) [positions[i * 2], positions[i * 2 + 1], fixed[i]] = [at.x, at.y, 1];
      });
      start.keep = { positions, fixed };
    }
    worker.postMessage(start);
    return () => {
      worker.terminate();
      if (live.current === state) live.current = null;
    };
  }, [graph]);

  // A live change: its new nodes settle among their neighbours; nothing else moves. Changes are applied only after
  // the first layout is done (GraphApp waits for it), so no position from that layout lands after this.
  const nudge = view?.nudge ?? null;
  useEffect(() => {
    const state = live.current;
    const v = args.current.view;
    if (!state || !v || !nudge || nudge.fresh.length === 0) return;
    const made = nudgeInput(v.graph, nudge.fresh);
    if (!made) return;
    state.nudges.set(nudge.n, { ids: made.ids, fixed: made.input.fixed });
    state.worker.postMessage({ kind: "nudge", id: nudge.n, ...made.input });
  }, [nudge]);

  // A new spacing (plan 198): laid out again from where the nodes are, or once the running layout ends.
  useEffect(() => {
    const state = live.current;
    if (!state) return;
    state.wanted = spacing;
    if (!state.laying && spacing !== state.spacing) state.relayout(spacing);
  }, [spacing]);

  useEffect(() => {
    const state = live.current;
    if (!state || !view || !visible || !edges) return;
    const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((v, i) => v === b[i]);
    const sameData = state.sent.index === view.index;
    if (sameData && same(state.sent.visible, visible) && same(state.sent.edges, edges) && state.sent.resolution === resolution) return;
    state.sent = { visible, edges, resolution, index: view.index };
    state.latest++;
    state.asked.set(state.latest, { resolution, ids: view.ids });
    // A request after a live change carries the graph; a queued one that replaces it keeps the newest graph.
    const graphNow = sameData ? state.pending?.graph : view.index;
    const request: WorkerCluster = { kind: "cluster", id: state.latest, visible, edges, resolution, seed: view.seed, ...(graphNow ? { graph: graphNow } : {}) };
    if (state.busy) {
      if (state.pending) state.asked.delete(state.pending.id);
      state.pending = request;
    } else {
      state.worker.postMessage(request);
      state.busy = true;
    }
  }, [view, visible, edges, resolution]);

  const layout = layoutFor && layoutFor.graph === view?.graph ? layoutFor.value : IDLE;
  const louvain = louvainFor && louvainFor.graph === view?.graph ? louvainFor.value : null;
  return { layout, louvain, nudged };
}

// The layout worker (design note section 1): Louvain and ForceAtlas2 off the main thread. "start" clusters the
// visible subgraph, posts the groups, seeds by them and lays out, posting positions as it goes; "cluster" reruns
// Louvain alone (a new resolution or visibility) on the graph it was started with, or the one a live change sent;
// "nudge" settles a live change's new nodes (plan 127); "relayout" runs the layout again from the positions on screen
// at a new spacing (plan 198).
import { type Groups, type IndexGraph, louvainGroups } from "@/lib/graph/clusters";
import { type Keep, layOut, nudge, runLayout } from "@/lib/graph/layout";

export type WorkerStart = {
  kind: "start";
  graph: IndexGraph;
  sizes: Float32Array;
  visible: Uint8Array;
  /** 1 for each edge whose relation is not filtered out. */
  edges: Uint8Array;
  resolution: number;
  seed: number;
  /** The Spacing slider's value: with the seed, what the layout is a function of (lib/graph/layout LayoutSeed). */
  spacing: number;
  budgetMs: number;
  /** The nodes already on screen when an agent opened more graphs (ws009 M1b1): they stay where they are. */
  keep?: Keep;
};
/** `graph`: the graph after a live change (plan 127); the worker clusters it from then on. */
export type WorkerCluster = { kind: "cluster"; id: number; visible: Uint8Array; edges: Uint8Array; resolution: number; seed: number; graph?: IndexGraph };
/** Settles new nodes (lib/graph/apply nudgeInput); the answer's positions are by the request's local index. */
export type WorkerNudge = { kind: "nudge"; id: number; n: number; edges: Uint32Array; positions: Float32Array; sizes: Float32Array; fixed: Uint8Array };
/** A new spacing (plan 198): the layout runs again, warm, from `positions` (by node index of `graph`). */
export type WorkerRelayout = { kind: "relayout"; graph: IndexGraph; positions: Float32Array; sizes: Float32Array; spacing: number; budgetMs: number };
export type WorkerRequest = WorkerStart | WorkerCluster | WorkerNudge | WorkerRelayout;

export type WorkerMessage =
  | { kind: "clusters"; id: number; groups: Groups; ms: number }
  | { kind: "tick"; positions: Float32Array; progress: number }
  | { kind: "done"; positions: Float32Array; iterations: number; ms: number; capped: boolean }
  | { kind: "nudged"; id: number; positions: Float32Array };

let graph: IndexGraph | null = null;
/** The newest groups, for a relayout's pull (lib/graph/layout compactGroups). */
let groups: Groups | null = null;

const post = (m: WorkerMessage, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(m, transfer);
const tick = (positions: Float32Array, progress: number) => post({ kind: "tick", positions, progress }, [positions.buffer]);

function cluster(id: number, visible: Uint8Array, edges: Uint8Array, resolution: number, seed: number): Groups {
  const started = performance.now();
  const found = louvainGroups(graph!, visible, resolution, seed, edges);
  groups = found;
  post({ kind: "clusters", id, groups: found, ms: Math.round(performance.now() - started) });
  return found;
}

self.onmessage = (event: MessageEvent<WorkerRequest>) => {
  const m = event.data;
  if (m.kind === "cluster") {
    if (m.graph) graph = m.graph;
    if (graph) cluster(m.id, m.visible, m.edges, m.resolution, m.seed);
    return;
  }
  if (m.kind === "nudge") {
    const positions = nudge(m);
    post({ kind: "nudged", id: m.id, positions }, [positions.buffer]);
    return;
  }
  if (m.kind === "relayout") {
    graph = m.graph;
    const result = runLayout(graph, m.sizes, m.positions, m.budgetMs, m.spacing, tick, undefined, groups ?? undefined);
    post({ kind: "done", ...result }, [result.positions.buffer]);
    return;
  }
  graph = m.graph;
  const found = cluster(0, m.visible, m.edges, m.resolution, m.seed);
  const result = layOut(graph, found, m.sizes, { seed: m.seed, spacing: m.spacing }, m.budgetMs, tick, m.keep);
  post({ kind: "done", ...result }, [result.positions.buffer]);
};

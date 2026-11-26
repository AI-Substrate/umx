import { type Groups, type IndexGraph, louvainGroups } from "@/lib/graph/clusters";
import { canonicalGraph, fanLayout, findHubs, hubSize, type HubOptions, type SimilarityLink } from "@/lib/graph/hubs";
import { type Keep, layOut, nudge } from "@/lib/graph/layout";

export type WorkerStart = {
  kind: "start";
  id: number;
  graph: IndexGraph;
  ids: readonly string[];
  sizes: Float32Array;
  visible: Uint8Array;
  edges: Uint8Array;
  resolution: number;
  seed: number;
  spacing: number;
  budgetMs: number;
  options: HubOptions;
  similarities: readonly SimilarityLink[];
  keep?: Keep;
};
export type WorkerCluster = Omit<WorkerStart, "kind"> & { kind: "cluster" };
export type WorkerNudge = { kind: "nudge"; id: number; n: number; edges: Uint32Array; positions: Float32Array; sizes: Float32Array; fixed: Uint8Array; spacing?: number };
export type WorkerRequest = WorkerStart | WorkerCluster | WorkerNudge;
export type HubResult = { centres: string[]; owner: [string, string][]; ms: number; fallback?: boolean };
export type WorkerMessage =
  | { kind: "clusters"; id: number; groups: Groups; ms: number }
  | { kind: "tick"; id: number; ids: readonly string[]; positions: Float32Array; progress: number }
  | { kind: "done"; id: number; ids: readonly string[]; positions: Float32Array; iterations: number; ms: number; capped: boolean; hubs: HubResult }
  | { kind: "nudged"; id: number; positions: Float32Array };

const post = (m: WorkerMessage, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(m, transfer);

self.onmessage = (event: MessageEvent<WorkerRequest>) => {
  const m = event.data;
  if (m.kind === "nudge") {
    const positions = nudge(m);
    post({ kind: "nudged", id: m.id, positions }, [positions.buffer]);
    return;
  }
  const started = performance.now();
  const canonical = canonicalGraph(m.graph, m.ids, m.visible, m.edges);
  const { graph, order, ids } = canonical;
  const found = louvainGroups(graph, new Uint8Array(graph.n).fill(1), m.resolution, m.seed);
  const groups: Groups = {
    of: new Int32Array(m.graph.n).fill(-1), sizes: found.sizes, heads: found.heads.map((i) => order[i]),
  };
  order.forEach((i, j) => { groups.of[i] = found.of[j]; });
  const clusterMs = Math.round(performance.now() - started);
  post({ kind: "clusters", id: m.id, groups, ms: clusterMs });
  if (m.kind === "cluster") return;
  const hubStart = performance.now();
  const plan = findHubs(graph, ids, m.options, m.resolution, m.seed, m.similarities);
  const hubs: HubResult = {
    centres: plan.centres.map((i) => ids[i]),
    owner: [],
    ms: Math.round(performance.now() - hubStart),
    fallback: plan.fallback,
  };
  plan.owner.forEach((head, i) => { if (head >= 0) hubs.owner.push([ids[i], ids[head]]); });
  // Hidden nodes need coordinates too: revealing one is a cluster-only update, not another full layout.
  const all = canonicalGraph(m.graph, m.ids, new Uint8Array(m.graph.n).fill(1), m.edges);
  const at = new Int32Array(m.graph.n);
  all.order.forEach((i, j) => { at[i] = j; });
  const wholePlan = { ...plan, centres: plan.centres.map((i) => at[order[i]]), owner: new Int32Array(all.graph.n).fill(-1) };
  plan.owner.forEach((head, i) => { if (head >= 0) wholePlan.owner[at[order[i]]] = at[order[head]]; });
  const wholeGroups: Groups = {
    of: Int32Array.from(all.order.map((i) => groups.of[i])), sizes: found.sizes, heads: found.heads.map((i) => at[order[i]]),
  };
  const centres = new Set(wholePlan.centres);
  const sizes = Float32Array.from(all.order.map((i, j) => centres.has(j) ? hubSize(m.sizes[i]) : m.sizes[i]));
  const keep = m.keep ? {
    positions: Float32Array.from(all.order.flatMap((i) => [m.keep!.positions[i * 2], m.keep!.positions[i * 2 + 1]])),
    fixed: Uint8Array.from(all.order.map((i) => m.keep!.fixed[i])),
  } : undefined;
  const result = layOut(all.graph, wholeGroups, sizes, { seed: m.seed, spacing: m.spacing }, Math.max(0, m.budgetMs - (performance.now() - started)), (positions, progress) => {
    post({ kind: "tick", id: m.id, ids: all.ids, positions, progress }, [positions.buffer]);
  }, keep);
  // An agent's open preserves existing positions; explicit layout changes run without keep.
  if (wholePlan.centres.length > 0 && !keep?.fixed.includes(1)) result.positions = fanLayout(result.positions, sizes, wholePlan, m.spacing);
  result.ms = Math.round(performance.now() - started);
  result.capped ||= result.ms >= m.budgetMs;
  post({ kind: "done", id: m.id, ids: all.ids, ...result, hubs }, [result.positions.buffer]);
};

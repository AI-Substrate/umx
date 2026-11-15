// Focus: the nodes within n hops of one node, over the visible graph. Mirrors `umx walk <id> --depth n
// --direction both` (umx-services query.rs Walk and hops): breadth first along every stored edge either way,
// similar_to left out, each node once at its shortest depth. Hidden nodes, and edges whose relation is filtered
// out, are not walked through.
import type { ViewGraph } from "@/lib/graph/build";

export const SIMILAR_TO = "similar_to";
const NONE: ReadonlySet<string> = new Set();

/** Node id -> depth for the start (0) and every visible node within `depth` hops of it. */
export function focusDepths(graph: ViewGraph, start: string, depth: number, offRelations: ReadonlySet<string> = NONE): Map<string, number> {
  const out = new Map<string, number>();
  if (!graph.hasNode(start) || graph.getNodeAttribute(start, "hidden")) return out;
  out.set(start, 0);
  let frontier = [start];
  for (let d = 1; d <= depth && frontier.length > 0; d++) {
    const next: string[] = [];
    for (const at of frontier) {
      graph.forEachEdge(at, (_e, a, source, target, sa, ta) => {
        if (a.kind === SIMILAR_TO || offRelations.has(a.kind)) return;
        const [other, oa] = source === at ? [target, ta] : [source, sa];
        if (oa.hidden || out.has(other)) return;
        out.set(other, d);
        next.push(other);
      });
    }
    frontier = next;
  }
  return out;
}

/** Focus as the app shows it. "unavailable": focus is asked for, but its node is hidden or gone, so nothing is
 * dimmed and no count is shown until the node is visible again (review R197-1). */
export type Focus =
  | { state: "off" }
  | { state: "unavailable" }
  | { state: "on"; depths: Map<string, number>; count: number };

export const FOCUS_OFF: Focus = { state: "off" };

export function focusOf(graph: ViewGraph, selected: string | null, depth: number, offRelations: ReadonlySet<string> = NONE): Focus {
  if (selected === null || depth <= 0) return FOCUS_OFF;
  const depths = focusDepths(graph, selected, depth, offRelations);
  if (!depths.has(selected)) return { state: "unavailable" };
  return { state: "on", depths, count: depths.size - 1 };
}

/** How the canvas draws a node under selection and focus. Focus forces labels for the selected node and hop 1 only;
 * hops 2 and 3 keep sigma's label grid, so a dense neighbourhood does not pile its labels up (ui-expert N9). */
export type Emphasis = "dim" | "selected" | "label" | "near" | "plain";

export function emphasis(node: string, selected: string | null, near: ReadonlySet<string>, focused: Map<string, number> | null): Emphasis {
  if (focused && !focused.has(node)) return "dim";
  if (node === selected) return "selected";
  if (near.has(node) || focused?.get(node) === 1) return "label";
  return focused ? "near" : "plain";
}

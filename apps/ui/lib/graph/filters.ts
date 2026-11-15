// Sidebar filters (req-111): chips for edge relations, node types and node statuses. Each set holds what is turned
// OFF, so a type or relation the graph gains later shows by default. A node is shown when the provenance and retired
// toggles allow it and neither its type nor its status is off; an edge when both ends are shown and its relation is
// not off. That one rule drives the canvas, the clusters, the hulls, focus, the legend and the counts.
import { PROVENANCE_TYPES, RETIRED_STATUSES } from "@/lib/graph/colors";

export type Visibility = { provenance: boolean; retired: boolean };
export type Filters = { relations: ReadonlySet<string>; types: ReadonlySet<string>; statuses: ReadonlySet<string> };
export type FilterDim = keyof Filters;

export const NO_FILTERS: Filters = { relations: new Set(), types: new Set(), statuses: new Set() };

export function anyOff(f: Filters): boolean {
  return f.relations.size > 0 || f.types.size > 0 || f.statuses.size > 0;
}

export function toggleChip(f: Filters, dim: FilterDim, key: string): Filters {
  const next = new Set(f[dim]);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return { ...f, [dim]: next };
}

type NodeLike = { kind: string; status: string };

/** What the toggles alone allow (provenance and retired). */
export function toggled(a: NodeLike, visible: Visibility): boolean {
  return (visible.provenance || !PROVENANCE_TYPES.has(a.kind)) && (visible.retired || !RETIRED_STATUSES.has(a.status));
}

export function nodeShown(a: NodeLike, visible: Visibility, f: Filters): boolean {
  return toggled(a, visible) && !f.types.has(a.kind) && !f.statuses.has(a.status);
}

/** Why a node is not on the canvas, in the order the sidebar names them; empty when it is shown. */
export type HiddenReason = "provenance" | "retired" | "type" | "status";

export function hiddenReasons(a: NodeLike, visible: Visibility, f: Filters): HiddenReason[] {
  const out: HiddenReason[] = [];
  if (!visible.provenance && PROVENANCE_TYPES.has(a.kind)) out.push("provenance");
  if (!visible.retired && RETIRED_STATUSES.has(a.status)) out.push("retired");
  if (f.types.has(a.kind)) out.push("type");
  if (f.statuses.has(a.status)) out.push("status");
  return out;
}

export function reasonText(a: NodeLike, reason: HiddenReason): string {
  switch (reason) {
    case "provenance":
      return "provenance";
    case "retired":
      return "out of play";
    case "type":
      return `type ${a.kind} filtered`;
    case "status":
      return `status ${a.status} filtered`;
  }
}

/** The toggles and filters with this node's reasons undone, and nothing else changed. */
export function reveal(a: NodeLike, visible: Visibility, f: Filters): { visible: Visibility; filters: Filters } {
  const types = new Set(f.types);
  const statuses = new Set(f.statuses);
  types.delete(a.kind);
  statuses.delete(a.status);
  return {
    visible: {
      provenance: visible.provenance || PROVENANCE_TYPES.has(a.kind),
      retired: visible.retired || RETIRED_STATUSES.has(a.status),
    },
    filters: { ...f, types, statuses },
  };
}

// The open set of graphs (ws009 M1a, plan 133; D69 (7)): which graphs a pick, "Open all" or a scope names, and which it
// leaves out. Pure, so any picker (the sidebar list today, the ui-expert's picker next) uses the same rules.
import { MAX_OPEN } from "@/lib/graph/keys";
import type { GraphSummary, ScopeSummary } from "@/lib/types";

/** Graphs a scope or "all" left out: counted, never named (d-20). */
export type LeftOut = { secured: number; unavailable: number };

/** `open` with `alias` added (at the end, so the others keep their colours) or taken out; never empty, never past
 * MAX_OPEN. */
export function toggled(open: readonly string[], alias: string): string[] {
  if (open.includes(alias)) return open.length > 1 ? open.filter((a) => a !== alias) : [...open];
  return open.length >= MAX_OPEN ? [...open] : [...open, alias];
}

/** What "All" opens: every loaded graph the picker would open that is not secured; secured ones are counted, never
 * named, and open only when picked or while "include secured graphs" is on (d-20, D69 (7)). The view never opens an
 * idle graph by itself, since that loads it. */
export function allGraphs(graphs: readonly GraphSummary[], includeSecured = false): { aliases: string[]; leftOut: LeftOut } {
  const aliases = [...new Set(graphs.filter((g) => g.state === "loaded" && g.openable && (includeSecured || !g.secured)).map((g) => g.alias))];
  const secured = includeSecured ? 0 : new Set(graphs.filter((g) => g.openable && g.secured).map((g) => g.id)).size;
  return { aliases: aliases.slice(0, MAX_OPEN), leftOut: { secured, unavailable: Math.max(0, aliases.length - MAX_OPEN) } };
}

/** What a scope opens: its open members, plus its secured ones only while "include secured graphs" is on. */
export function scopeGraphs(s: ScopeSummary, includeSecured: boolean): { aliases: string[]; leftOut: LeftOut } {
  const all = includeSecured ? [...s.graphs, ...s.securedGraphs] : s.graphs;
  const secured = includeSecured ? s.secured - s.securedGraphs.length : s.secured;
  return { aliases: all.slice(0, MAX_OPEN), leftOut: { secured, unavailable: s.unavailable + Math.max(0, all.length - MAX_OPEN) } };
}

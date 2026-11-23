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

/** The picker's sections: what opens now, what opening loads first, and what cannot open (with its reason). */
export type PickGroup = "loaded" | "idle" | "blocked";
export function groupOf(g: GraphSummary): PickGroup {
  return !g.openable ? "blocked" : g.state === "loaded" ? "loaded" : "idle";
}

/** The graphs whose alias holds every word of `query` (any case, any order), in umx's order. */
export function matchGraphs<G extends Pick<GraphSummary, "alias">>(graphs: readonly G[], query: string): G[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...graphs];
  return graphs.filter((g) => {
    const a = g.alias.toLowerCase();
    return words.every((w) => a.includes(w));
  });
}

/** A draft selection with `alias` added (at the end) or taken out. Unlike `toggled` it may become empty: an empty
 * draft clears the canvas. Never past MAX_OPEN. */
export function draftToggled(draft: readonly string[], alias: string): string[] {
  if (draft.includes(alias)) return draft.filter((a) => a !== alias);
  return draft.length >= MAX_OPEN ? [...draft] : [...draft, alias];
}

/** The draft plus every loaded graph in `matching` that "Open all" would take, up to MAX_OPEN; secured ones only while
 * "include secured graphs" is on, else counted (never named), and those past the cap counted as unavailable. */
export function selectMatching(draft: readonly string[], matching: readonly GraphSummary[], includeSecured: boolean): { aliases: string[]; leftOut: LeftOut } {
  const picked = allGraphs(matching, includeSecured);
  const wanted = [...new Set([...draft, ...matching.filter((g) => g.state === "loaded" && g.openable && (includeSecured || !g.secured)).map((g) => g.alias)])];
  return { aliases: wanted.slice(0, MAX_OPEN), leftOut: { secured: picked.leftOut.secured, unavailable: Math.max(0, wanted.length - MAX_OPEN) } };
}

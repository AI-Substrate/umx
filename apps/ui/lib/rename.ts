// The view follows a graph rename (plan 215, after `umx graph rename`): the open set and everything keyed by alias
// move from the former alias to the live one in one step. The graph-addressing expert's rules hold: the live alias is
// what umx resolved (never walked here), the mapping lives in memory only, identity is the graph id, and only a graph
// in the open set switches.
import { aliasOfKey, qid, unq } from "@/lib/graph/keys";
import type { Selection } from "@/lib/selection";

export type Rename = { from: string; to: string; id: string };

/**
 * The open set after `r`, or null when it does not switch: `from` must be in the open set (a rename of any other alias
 * changes nothing, rule 6), and the graph open there must be the one umx resolved, by graph id (rule 3). The live alias
 * takes the former one's place in the open order, so the graph keeps its colour; when it is already open too, the two
 * are one graph and become one tab.
 */
export function renameTarget(open: readonly string[], ids: ReadonlyMap<string, string>, r: Rename): string[] | null {
  if (!open.includes(r.from) || ids.get(r.from) !== r.id) return null;
  return open.includes(r.to) ? open.filter((a) => a !== r.from) : open.map((a) => (a === r.from ? r.to : a));
}

/** A canvas key (`<alias>/<id>`) under the live alias when its graph was renamed; any other key as it is. */
export function renameKey(key: string, renames: ReadonlyMap<string, string>): string {
  const to = renames.get(aliasOfKey(key));
  return to === undefined ? key : qid(to, unq(key).id);
}

/**
 * The renames still being followed once `r` comes (review R404-1): every earlier one stays, one whose live alias `r`
 * renames again now leads to `r`'s live alias, and `r` joins. A view that has not yet read the first rename's graph
 * keeps that graph's selection and positions through the second.
 */
export function composeRenames(pending: ReadonlyMap<string, string>, r: { from: string; to: string }): Map<string, string> {
  const out = new Map(pending);
  for (const [from, to] of out) if (to === r.from) out.set(from, r.to);
  out.set(r.from, r.to);
  return out;
}

/** The graph ids of the open set, with each pending rename's live alias standing for the graph it renamed. */
export function idsWithPending(ids: ReadonlyMap<string, string>, pending: ReadonlyMap<string, string>): Map<string, string> {
  const out = new Map(ids);
  for (const [from, to] of pending) if (out.has(from) && !out.has(to)) out.set(to, out.get(from)!);
  return out;
}

/** Whether an open set that follows `renames` may land: every live alias it wants is on screen or has arrived, so no
 * renamed graph's nodes leave the screen (and the selection) before their replacement is there. */
export function renamesLanded(renames: ReadonlyMap<string, string>, wanted: readonly string[], onScreen: readonly string[], arrived: readonly string[]): boolean {
  return [...renames.values()].every((to) => !wanted.includes(to) || onScreen.includes(to) || arrived.includes(to));
}

/** The selection on the open set `key` once it lands: each node under its live alias, kept while the canvas has it. */
export function movedSelection(sel: Selection, key: string, renames: ReadonlyMap<string, string>, has: (id: string) => boolean): Selection {
  const primary = sel.primary === null ? null : renameKey(sel.primary, renames);
  return { graph: key, ids: sel.ids.map((id) => renameKey(id, renames)).filter(has), primary: primary !== null && has(primary) ? primary : null };
}

/** What the view says once per rename: a quiet notice, never a hint card per read. */
export function renameNotice(r: { from: string; to: string }) {
  return { code: "ui_graph_renamed", message: `${r.from} was renamed to ${r.to}.`, next: null };
}

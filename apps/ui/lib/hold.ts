// Live and Hold (ws009 M1d, plan 152; ui-expert design 2026-11-12): what each open graph's change feed told the view
// and the view has not applied yet. Live applies it at once; Hold keeps it, counted per graph, until the person applies
// a graph or all of them. Both go through the one apply path (GraphApp applyChange), so a held change applied later is
// exactly the change Live would have applied. Pure, so tests run every rule directly.
import { unq } from "@/lib/graph/keys";
import { type LiveCursor, NOTHING, over, plan, type TabChange, thenTouched, type Touched } from "@/lib/live";

/**
 * One graph's changes not yet applied: from the revision on screen (`from`) to the last one the feed told of
 * (`seen`). `revs` counts the feed's revisions in between, one per write, a write that touched no id too. `touched`
 * is null when the ids are not known (an overflow, a feed reset, more than MAX_CHANGE_IDS ids): applying then reloads
 * the graph whole.
 */
export type Held = { from: LiveCursor; seen: LiveCursor; revs: number; touched: Touched | null };

/** Nothing held: the view is at `at`. */
export function heldAt(at: LiveCursor): Held {
  return { from: at, seen: at, revs: 0, touched: NOTHING };
}

/** A change the feed sent for the revision the view last saw: folded in, skipped (a duplicate or an out-of-order
 * answer, lib/live plan), or, when it cannot follow on (an overflow, another epoch), the ids are no longer known. */
export function heldReceive(h: Held, epoch: string, change: TabChange): Held {
  const how = plan(h.seen, epoch, change);
  if (how === "skip") return h;
  if (how === "reload" || change.kind === "overflow") {
    const rev = change.kind === "overflow" ? change.rev : change.to;
    return { ...h, seen: { epoch, rev }, touched: null };
  }
  const touched = h.touched === null ? null : thenTouched(h.touched, change);
  return { ...h, seen: { epoch, rev: change.to }, revs: h.revs + (change.to - change.from), touched: touched && !over(touched) ? touched : null };
}

/** `a` followed by `later` (which starts where `a` was seen): what is owed when an apply of `a` failed while `later`
 * was collected. */
export function heldThen(a: Held, later: Held): Held {
  const touched = a.touched && later.touched ? thenTouched(a.touched, later.touched) : null;
  return { from: a.from, seen: later.seen, revs: a.revs + later.revs, touched: touched && !over(touched) ? touched : null };
}

/** Whether anything is owed: a revision past the one on screen, or ids not known. */
export function owes(h: Held): boolean {
  return h.touched === null || h.seen.rev !== h.from.rev || h.seen.epoch !== h.from.epoch;
}

/** What applying `h` asks of the one apply path: the changes merged, or a whole reload when their ids are not known
 * or the feed restarted since the revision on screen. Null when nothing is owed. */
export function heldChange(h: Held): { change: TabChange; how: "apply" | "reload"; epoch: string } | null {
  if (!owes(h)) return null;
  const epoch = h.seen.epoch ?? "";
  if (h.touched === null || h.seen.epoch !== h.from.epoch) return { change: { kind: "overflow", rev: h.seen.rev }, how: "reload", epoch };
  return { change: { kind: "changed", from: h.from.rev, to: h.seen.rev, ...h.touched }, how: "apply", epoch };
}

/** The number to show for a graph: its held revisions, or null ("many") when that is not known, which is never 0: an
 * overflow, a feed reset, or a revision on screen the feed has not placed yet. */
export function heldCount(h: Held | undefined): number | null {
  if (!h || h.seen.epoch === null || h.touched === null || h.seen.epoch !== h.from.epoch) return null;
  return h.revs;
}

/** All the open graphs' held revisions, or null ("many") when any graph's count is not known. */
export function heldTotal(held: ReadonlyMap<string, Held>, aliases: readonly string[]): number | null {
  let n = 0;
  for (const a of aliases) {
    const c = heldCount(held.get(a));
    if (c === null) return null;
    n += c;
  }
  return n;
}

export const countText = (n: number | null) => (n === null ? "many" : String(n));

/** Whether node `id` (a plain umx id) of a graph may exist only in its held changes: added or re-added there, or the
 * held ids are not known. The caller has checked it is not on the canvas. */
export function heldMayHave(h: Held | undefined, id: string): boolean {
  if (!h || !owes(h)) return false;
  if (h.touched === null) return true;
  return h.touched.nodes.added.includes(id) || h.touched.nodes.edited.includes(id);
}

/**
 * The open graph whose held changes an agent's command named a node in (plan 152): a node not on the canvas that may
 * exist only in that command's own graph's held changes. Events carry canvas keys (lib/agent qualifyEvent). The
 * command is not applied by itself: the view says so and offers that graph's Apply. Null when there is none.
 */
export function namedHeld(
  events: readonly { graph: string; command: { kind: string; node?: string; nodes?: string[] } }[],
  held: ReadonlyMap<string, Held>,
  onCanvas: (key: string) => boolean,
): string | null {
  let out: string | null = null;
  for (const e of events) {
    const c = e.command;
    const named = c.kind === "focus" && c.node ? [c.node] : c.kind === "highlight" || c.kind === "fit" ? (c.nodes ?? []) : [];
    if (named.some((key) => !onCanvas(key) && heldMayHave(held.get(e.graph), unq(key).id))) out = e.graph;
  }
  return out;
}

/** What the strip says while held, for the open graphs it was said for (`set`, lib/graph/keys setKey): an agent named
 * a node only in a graph's held changes, or an apply took the selected node off. */
export type HoldNote = { set: string; kind: "pending"; alias: string } | { set: string; kind: "removed" };

/**
 * The note to show for the open graphs now, and the graph its Apply applies (review B2): none once the open
 * graphs changed, and no pending note for a graph that is not open, so a note and its Apply never outlive their graphs.
 */
export function noteFor(note: HoldNote | null, open: { set: string; aliases: readonly string[] }): { text: string; apply: string | null } | null {
  if (!note || note.set !== open.set) return null;
  if (note.kind === "removed") return { text: "your selected node was removed", apply: null };
  return open.aliases.includes(note.alias) ? { text: `agent named a node in pending changes (${note.alias})`, apply: note.alias } : null;
}

/** The view's mode, as the browser keeps it: a name only, like the theme. Held revisions are never kept: a reload
 * reads the graphs as they are now and starts with nothing held. */
export type LiveMode = "live" | "hold";
export const MODE_KEY = "live-mode";
export function liveMode(value: unknown): LiveMode {
  return value === "hold" ? "hold" : "live";
}

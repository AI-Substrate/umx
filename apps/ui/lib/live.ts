// Live updates (ws009 S5b, plan 127): the change feed S5a added to UiWait (umx docs get ui, "The change feed"), as
// pure functions the view's server and the browser share. The server's one UiWait follows a revision per open graph
// and keeps the last changes it was told; each tab says which revision it applied and gets what changed since,
// merged into one, or an overflow (reload the graph whole). Ids and revisions only, never text (security T1/T2).

/** umx-core ui MAX_CHANGE_IDS: past this many ids in one list, a change is an overflow. */
export const MAX_CHANGE_IDS = 500;
/** umx-core ui CHANGE_LOG: the changes kept per graph; a tab further behind reloads the graph. */
export const CHANGE_LOG = 64;

export type Ids = { added: string[]; edited: string[]; removed: string[] };
export type Touched = { nodes: Ids; edges: Ids };

/** What a tab applies: the changes from its revision to `to`, merged, or "reload the graph whole, then wait from `rev`". */
export type TabChange = { kind: "changed"; from: number; to: number; nodes: Ids; edges: Ids } | { kind: "overflow"; rev: number };

/** Where a tab's graph is: the revision it applied, in the hub epoch it was told it in. `epoch: null` knows none. */
export type LiveCursor = { epoch: string | null; rev: number };

export const NO_IDS: Ids = { added: [], edited: [], removed: [] };
export const NOTHING: Touched = { nodes: NO_IDS, edges: NO_IDS };

/**
 * `a` followed by `later`, as one (umx-core TouchedIds::then): an id added and then edited is added; added and then
 * removed is gone from both (the reader never had it); removed and then added again is edited.
 */
export function thenIds(a: Ids, later: Ids): Ids {
  const added = new Set(a.added);
  const edited = new Set(a.edited);
  const removed = new Set(a.removed);
  for (const id of later.added) {
    if (removed.delete(id)) edited.add(id);
    else added.add(id);
  }
  for (const id of later.edited) if (!added.has(id)) edited.add(id);
  for (const id of later.removed) {
    if (!added.delete(id)) {
      edited.delete(id);
      removed.add(id);
    }
  }
  return { added: [...added], edited: [...edited], removed: [...removed] };
}

export function thenTouched(a: Touched, later: Touched): Touched {
  return { nodes: thenIds(a.nodes, later.nodes), edges: thenIds(a.edges, later.edges) };
}

const overIds = (i: Ids) => i.added.length > MAX_CHANGE_IDS || i.edited.length > MAX_CHANGE_IDS || i.removed.length > MAX_CHANGE_IDS;
export const over = (t: Touched) => overIds(t.nodes) || overIds(t.edges);

/** One graph's revisions as the server's UiWait follows them: its revision, and the steps that led there. */
type Followed = { rev: number; floor: number; steps: { from: number; to: number; touched: Touched }[] };

/** What the daemon said changed in one graph (lib/umx/socket readWaited). */
export type DaemonChange =
  | { graph: string; overflow: false; from_rev: number; to_rev: number; nodes: Ids; edges: Ids }
  | { graph: string; overflow: true; rev: number };

/**
 * The server's record of each followed graph, for one daemon epoch: the revision it sends in the next UiWait, and the
 * last CHANGE_LOG steps, so tabs at different revisions each get what they lack. A change that does not follow the
 * revision the server has (a gap, which the daemon never sends) counts as an overflow: nothing is ever skipped.
 */
export class Revisions {
  private graphs = new Map<string, Followed>();

  /** The revision to send for `graph`: 0 for one not followed yet, which the daemon answers with an overflow. */
  rev(graph: string): number {
    return this.graphs.get(graph)?.rev ?? 0;
  }

  has(graph: string): boolean {
    return this.graphs.has(graph);
  }

  /** Starts following `graph` at 0 (nothing known), unless it is followed already. */
  follow(graph: string) {
    if (!this.graphs.has(graph)) this.graphs.set(graph, { rev: 0, floor: 0, steps: [] });
  }

  /** Stops following every graph not in `keep`. */
  keep(keep: ReadonlySet<string>) {
    for (const g of [...this.graphs.keys()]) if (!keep.has(g)) this.graphs.delete(g);
  }

  /** A new daemon epoch: every revision was the old daemon's. */
  clear() {
    this.graphs.clear();
  }

  record(c: DaemonChange) {
    const f = this.graphs.get(c.graph);
    if (!f) return;
    if (c.overflow) {
      this.graphs.set(c.graph, { rev: c.rev, floor: c.rev, steps: [] });
      return;
    }
    // A duplicate or an older answer: already recorded.
    if (c.to_rev <= f.rev) return;
    if (c.from_rev !== f.rev) {
      this.graphs.set(c.graph, { rev: c.to_rev, floor: c.to_rev, steps: [] });
      return;
    }
    f.steps.push({ from: c.from_rev, to: c.to_rev, touched: { nodes: c.nodes, edges: c.edges } });
    if (f.steps.length > CHANGE_LOG) f.steps.shift();
    f.floor = f.steps[0].from;
    f.rev = c.to_rev;
  }

  /** What a tab at `at` lacks for `graph`: nothing, the changes since merged, or an overflow. Null when not followed. */
  since(graph: string, epoch: string, at: LiveCursor): TabChange | null {
    const f = this.graphs.get(graph);
    if (!f) return null;
    const overflow: TabChange = { kind: "overflow", rev: f.rev };
    if (at.epoch !== epoch) return overflow;
    if (at.rev === f.rev) return null;
    if (at.rev < f.floor || at.rev > f.rev) return overflow;
    let merged = NOTHING;
    for (const s of f.steps) if (s.from >= at.rev) merged = thenTouched(merged, s.touched);
    if (over(merged)) return overflow;
    return { kind: "changed", from: at.rev, to: f.rev, ...merged };
  }
}

/**
 * What a tab does with a change for its graph at `at`: apply it, reload the graph whole, or skip it (a duplicate or an
 * out-of-order answer for a revision it is not at). So no revision is applied twice, and none is skipped: a change
 * that starts anywhere but `at` is never applied over a gap.
 */
export function plan(at: LiveCursor, epoch: string, change: TabChange): "apply" | "reload" | "skip" {
  if (change.kind === "overflow") return at.epoch === epoch && at.rev === change.rev ? "skip" : "reload";
  if (at.epoch !== epoch) return "reload";
  return change.from === at.rev && change.to > at.rev ? "apply" : "skip";
}

/**
 * Whether the view is live (review B6): every open graph's revision is known and no open graph's last change
 * failed. `failed` holds each graph whose change could not be applied, until that graph's next change applies.
 */
export function liveAll(revs: readonly (LiveCursor | null)[], failed: ReadonlyMap<string, unknown>, open: readonly string[]): boolean {
  return revs.length > 0 && revs.every((r) => r !== null) && !open.some((a) => failed.has(a));
}

/** The node ids and edge ids to read for a change: every id added or edited (removed ones are dropped, not read). */
export function toRead(t: Touched): { nodes: string[]; edges: string[] } {
  return { nodes: [...t.nodes.added, ...t.nodes.edited], edges: [...t.edges.added, ...t.edges.edited] };
}

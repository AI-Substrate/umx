// The person's selection and how it reaches agents (ws009 S7, plan 117). Click selects one node, shift-click adds or
// removes one, a shift-drag lasso adds the shown nodes inside it, Esc clears focus and then the selection. The tab
// publishes the selection to the daemon's memory through the view's server (POST /api/graphs/<alias>/selection),
// debounced, never continuously; `umx ui selection` reads it. A pause toggle (per tab, on at start) stops publishing
// and clears the daemon's copy at once (security T4b).
import type { ApiError } from "@/lib/types";

/** The most nodes one selection holds (umx-core ui MAX_SELECTION). */
export const MAX_SELECTION = 1_000;
/** How long the selection must stay still before it is published. */
export const DEBOUNCE_MS = 250;

/**
 * What the person selected in the open graphs (`graph` is their set key, lib/graph/keys setKey), as canvas keys, in the
 * order they selected it. `primary` is the node the panel shows and focus starts from: the last one clicked, or none
 * (a lasso adds nodes without opening the panel).
 */
export type Selection = { graph: string | null; ids: readonly string[]; primary: string | null };

export function noSelection(graph: string | null): Selection {
  return { graph, ids: [], primary: null };
}

/** A click: that node alone. */
export function pickOne(s: Selection, id: string): Selection {
  return { graph: s.graph, ids: [id], primary: id };
}

/** A shift-click: adds the node (it becomes the panel's), or removes it if selected. Nothing is added past the cap. */
export function toggleOne(s: Selection, id: string): Selection {
  if (s.ids.includes(id)) {
    return { graph: s.graph, ids: s.ids.filter((x) => x !== id), primary: s.primary === id ? null : s.primary };
  }
  if (s.ids.length >= MAX_SELECTION) return s;
  return { graph: s.graph, ids: [...s.ids, id], primary: id };
}

/** A lasso: adds the nodes not yet selected, up to the cap. `left` counts the new ones the cap kept out. */
export function addMany(s: Selection, ids: readonly string[]): { next: Selection; left: number } {
  const have = new Set(s.ids);
  const fresh = [...new Set(ids)].filter((id) => !have.has(id));
  const room = Math.max(0, MAX_SELECTION - s.ids.length);
  const added = fresh.slice(0, room);
  const next = added.length === 0 ? s : { graph: s.graph, ids: [...s.ids, ...added], primary: s.primary };
  return { next, left: fresh.length - added.length };
}

/** Nodes a live change removed leave the selection (plan 127). `primaryGone`: the panel's node was one of them, so
 * the panel says it was removed instead of closing under the person. */
export function dropRemoved(s: Selection, removed: ReadonlySet<string>): { next: Selection; primaryGone: boolean } {
  const ids = s.ids.filter((id) => !removed.has(id));
  const primaryGone = s.primary !== null && removed.has(s.primary);
  if (ids.length === s.ids.length && !primaryGone) return { next: s, primaryGone };
  return { next: { graph: s.graph, ids, primary: primaryGone ? null : s.primary }, primaryGone };
}

/**
 * Takes removed nodes out of the selection held in `now` and advances `now` at once, so the next graph's removals in
 * the same poll start from this result, not from a selection React has not committed yet (review B4).
 */
export function removeFromSelection(now: { current: Selection }, removed: ReadonlySet<string>): { before: Selection; next: Selection; primaryGone: boolean } {
  const before = now.current;
  const dropped = dropRemoved(before, removed);
  now.current = dropped.next;
  return { before, ...dropped };
}

/** "This node was removed": the panel's node a live change took off, shown while the selection is `after`. */
export type RemovalNotice = { id: string; after: Selection } | null;

/**
 * The notice after a live change pruned the selection (removeFromSelection's result). A change that removed the
 * panel's node starts one; a later automatic pruning (another graph's removals in the same poll, say) carries it on
 * to the new selection, so it never vanishes under the person (review B4 round 2). Any other change to the
 * selection, such as a person's click, leaves `after` behind and so dismisses it (noticeShown).
 */
export function noticeAfterPrune(notice: RemovalNotice, pruned: { before: Selection; next: Selection; primaryGone: boolean }): RemovalNotice {
  if (pruned.primaryGone && pruned.before.primary) return { id: pruned.before.primary, after: pruned.next };
  if (notice && notice.after === pruned.before && pruned.next !== pruned.before) return { ...notice, after: pruned.next };
  return notice;
}

/** The removed node the panel reports, or null once the selection moved on from the notice's. */
export function noticeShown(notice: RemovalNotice, selection: Selection): string | null {
  return notice && notice.after === selection ? notice.id : null;
}

/** Closing the panel keeps the selection; only the panel's node is let go. */
export function closePanel(s: Selection): Selection {
  return s.primary === null ? s : { ...s, primary: null };
}

/** Esc: focus first, then the whole selection. */
export function escape(focusDepth: number, s: Selection): { focusDepth: number; selection: Selection } {
  if (focusDepth > 0) return { focusDepth: 0, selection: s };
  return { focusDepth: 0, selection: s.ids.length === 0 && s.primary === null ? s : noSelection(s.graph) };
}

/** Whether (x, y) is inside the polygon `poly` (flat x0, y0, x1, y1, ...), by the even-odd rule. */
export function inPolygon(poly: readonly number[], x: number, y: number): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 2; i < poly.length; j = i, i += 2) {
    const [xi, yi, xj, yj] = [poly[i], poly[i + 1], poly[j], poly[j + 1]];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/** The lasso's nodes: those shown whose centre (in the same screen coordinates as `poly`) is inside it. */
export function nodesInLasso(poly: readonly number[], nodes: Iterable<{ id: string; x: number; y: number; hidden: boolean }>): string[] {
  if (poly.length < 6) return [];
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (let i = 0; i < poly.length; i += 2) {
    [x0, x1] = [Math.min(x0, poly[i]), Math.max(x1, poly[i])];
    [y0, y1] = [Math.min(y0, poly[i + 1]), Math.max(y1, poly[i + 1])];
  }
  const out: string[] = [];
  for (const n of nodes) {
    if (n.hidden || n.x < x0 || n.x > x1 || n.y < y0 || n.y > y1) continue;
    if (inPolygon(poly, n.x, n.y)) out.push(n.id);
  }
  return out;
}

/** A publish for `graph` that failed and has not succeeded since: agents may still see that graph's earlier selection. */
export type ShareFailure = { graph: string; error: ApiError };

/** What the top bar shows: whether the tab shares its selection, and every graph whose last publish failed. */
export type ShareState = { sharing: boolean; failures: readonly ShareFailure[] };

/** How long a failed publish waits before it is tried again, doubling up to RETRY_MAX_MS. */
export const RETRY_MIN_MS = 2_000;
export const RETRY_MAX_MS = 30_000;

/** Matches no list of ids (ids are Crockford characters). */
const UNKNOWN = "?";

export type PublishDeps = {
  /** Publishes `nodes` as the selection in `graph` (none clears it); rejects with the reason it failed. */
  post: (graph: string, nodes: readonly string[]) => Promise<void>;
  changed: (s: ShareState) => void;
  setTimer: (fn: () => void, ms: number) => unknown;
  clearTimer: (t: unknown) => void;
  asIssue: (e: unknown) => ApiError;
};

type Job = { graph: string; nodes: readonly string[] };

/** The selection as the daemon keeps it: one list of plain ids per open graph, every open graph present (plan 133). */
export function perGraph(aliases: readonly string[], keys: readonly string[]): Map<string, string[]> {
  const out = new Map<string, string[]>(aliases.map((a) => [a, []]));
  for (const key of keys) {
    const i = key.indexOf("/");
    out.get(key.slice(0, i))?.push(key.slice(i + 1));
  }
  return out;
}

/**
 * Publishes one tab's selection. The daemon keeps one selection per graph, shared by every tab and view, so this tab
 * never assumes what it holds. An ordinary change waits DEBOUNCE_MS for the next one, so a burst is one publish per
 * graph, and is skipped for a graph when it equals what this tab last sent there (a graph it never sent to counts as
 * empty, so opening a graph never clears another tab's selection). Pausing (empty), unpausing (the current selection,
 * even empty) and a graph leaving the view (empty for that graph, before anything for one that joins) are always sent
 * (review R235-1). Publishes go out one at a time in order, and a queued one for the same graph is replaced by a newer
 * one. A failed publish stays in `failures` for its graph until a publish for that graph succeeds, and is retried with
 * a backoff (review R235-2).
 */
export class Publisher {
  private sharing = true;
  /** Every open graph and its selected ids (empty lists included). */
  private want = new Map<string, readonly string[]>();
  private timer: unknown = null;
  /** Per graph, what this tab last queued (ids joined); UNKNOWN after a failure. */
  private last = new Map<string, string>();
  private queue: Job[] = [];
  private busy = false;
  private failed = new Map<string, ApiError>();
  /** Per graph: the retry timer waiting, and the last retry delay (reset by a success). */
  private retries = new Map<string, unknown>();
  private backoff = new Map<string, number>();
  /** Graphs being switched to their live alias (plan 215): former alias to live alias, only until the screen stops
   * naming the former one, as it may later be another graph's live alias (graph-addressing expert rule 2). */
  private renamed = new Map<string, string>();

  constructor(private readonly deps: PublishDeps) {}

  /** The selection on screen now: every open graph and its ids. One graph and its ids also works (`update(g, ids)`). */
  update(graphs: ReadonlyMap<string, readonly string[]> | string | null, ids: readonly string[] = []): void {
    const given: ReadonlyMap<string, readonly string[]> = graphs === null ? new Map() : typeof graphs === "string" ? new Map([[graphs, ids]]) : graphs;
    // A former alias still on screen while the view switches is published under its live alias.
    const next = new Map<string, readonly string[]>();
    for (const [g, list] of given) {
      const live = this.renamed.get(g) ?? g;
      next.set(live, [...new Set([...(next.get(live) ?? []), ...list])]);
    }
    for (const g of [...this.renamed.keys()]) if (!given.has(g)) this.renamed.delete(g);
    const left = [...this.want.keys()].filter((g) => !next.has(g));
    const joined = [...next.keys()].some((g) => !this.want.has(g));
    this.want = next;
    if (left.length > 0 || joined) {
      this.cancel();
      for (const g of left) this.enqueue({ graph: g, nodes: [] }, true);
      // Every open graph then gets what is on screen now: one this tab published to before is cleared if the screen
      // holds nothing there (plan 133 review), and one it never sent to is left as it is.
      this.schedule();
      return;
    }
    this.schedule();
  }

  /**
   * A graph open here was renamed (plan 215): what this tab publishes there moves to the live alias, and nothing is
   * ever sent through the former one again. umx would read a selection sent through it as the live graph's, so a
   * clear there could undo the live publish; the daemon's entry under the former alias names no loaded graph and
   * shows nothing. The live alias is published to afresh.
   */
  rename(from: string, to: string): void {
    if (from === to) return;
    for (const [k, v] of this.renamed) if (v === from) this.renamed.set(k, to);
    this.renamed.set(from, to);
    if (this.retries.has(from)) this.deps.clearTimer(this.retries.get(from));
    this.retries.delete(from);
    this.backoff.delete(from);
    this.failed.delete(from);
    this.last.delete(from);
    this.queue = this.queue.filter((q) => q.graph !== from);
    const was = this.want.get(from);
    if (was !== undefined) {
      this.want.delete(from);
      this.want.set(to, [...new Set([...(this.want.get(to) ?? []), ...was])]);
    }
    this.last.set(to, UNKNOWN);
    this.emit();
    this.schedule();
  }

  setSharing(on: boolean): void {
    if (on === this.sharing) return;
    this.sharing = on;
    this.cancel();
    for (const [graph, nodes] of this.want) this.enqueue({ graph, nodes: on ? nodes : [] }, true);
    this.emit();
  }

  /** Drops a publish still waiting for its debounce. */
  cancel(): void {
    if (this.timer !== null) this.deps.clearTimer(this.timer);
    this.timer = null;
  }

  /** The tab is going away: drops every timer, debounce and retries alike. */
  stop(): void {
    this.cancel();
    for (const t of this.retries.values()) this.deps.clearTimer(t);
    this.retries.clear();
  }

  private schedule() {
    this.cancel();
    if (!this.sharing || this.want.size === 0) return;
    // Pausing and changing the open graphs cancel this timer first.
    this.timer = this.deps.setTimer(() => {
      this.timer = null;
      for (const [graph, nodes] of this.want) this.enqueue({ graph, nodes }, false);
    }, DEBOUNCE_MS);
  }

  /** Queues a publish; `force` sends it even when it matches what this tab last sent. */
  private enqueue(job: Job, force: boolean) {
    if (this.renamed.has(job.graph)) return;
    const key = job.nodes.join(",");
    if (!force && (this.last.get(job.graph) ?? "") === key) return;
    this.last.set(job.graph, key);
    // A newer publish for the graph replaces a retry still waiting.
    if (this.retries.has(job.graph)) {
      this.deps.clearTimer(this.retries.get(job.graph));
      this.retries.delete(job.graph);
    }
    const queued = this.queue.find((q) => q.graph === job.graph);
    if (queued) queued.nodes = job.nodes;
    else this.queue.push({ ...job });
    void this.drain();
  }

  private async drain() {
    if (this.busy) return;
    this.busy = true;
    try {
      for (let job = this.queue.shift(); job; job = this.queue.shift()) {
        try {
          await this.deps.post(job.graph, job.nodes);
          this.failed.delete(job.graph);
          this.backoff.delete(job.graph);
        } catch (e) {
          const issue = this.deps.asIssue(e);
          // The graph was renamed (plan 215): the view switches it, and this tab never publishes through the former
          // alias again, so nothing is retried or reported for it.
          if (issue.code === "ui_graph_renamed") {
            this.last.delete(job.graph);
            this.failed.delete(job.graph);
            this.backoff.delete(job.graph);
          } else {
            // The daemon's copy is unknown now: the next publish for this graph goes out, even an empty one.
            this.last.set(job.graph, UNKNOWN);
            this.failed.set(job.graph, issue);
            this.retryLater(job);
          }
        }
        this.emit();
      }
    } finally {
      this.busy = false;
    }
  }

  /** Sends `job` again after a backoff, unless a newer publish for its graph comes first. */
  private retryLater(job: Job) {
    if (this.queue.some((q) => q.graph === job.graph)) return;
    const delay = Math.min(Math.max(RETRY_MIN_MS, (this.backoff.get(job.graph) ?? 0) * 2), RETRY_MAX_MS);
    this.backoff.set(job.graph, delay);
    const timer = this.deps.setTimer(() => {
      this.retries.delete(job.graph);
      // What the graph should hold now: the screen's selection (none while paused), or empty for a graph left.
      const open = this.want.get(job.graph);
      const nodes = open !== undefined ? (this.sharing ? open : []) : job.nodes;
      this.enqueue({ graph: job.graph, nodes }, true);
    }, delay);
    this.retries.set(job.graph, timer);
  }

  private emit() {
    this.deps.changed({ sharing: this.sharing, failures: [...this.failed].map(([graph, error]) => ({ graph, error })) });
  }
}

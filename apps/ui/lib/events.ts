// Agent events for the browser (ws009 S6b, plan 114). The Next server holds ONE UiWait loop on the daemon, naming the
// graphs the view's tabs have open, and answers every tab from what it keeps: each tab long-polls GET /api/events
// with its cursor (security T3: fetch through the usual guard, no SSE, no CORS). Events carry ids only; the browser
// resolves titles from the graph it already read. The same loop follows each open graph's revision (the change feed,
// S5b plan 127, lib/live): a tab sends the revision it applied and gets what changed since, or an overflow.
import { type LiveCursor, Revisions, type TabChange } from "@/lib/live";
import type { AgentEvent, Waited, WaitArgs } from "@/lib/umx/socket";
import { UmxError } from "@/lib/umx";

/** How long a tab's poll is held when nothing happens, under any proxy or browser timeout. */
export const HOLD_MS = 20_000;
/** Each daemon wait: the daemon's longest. */
export const WAIT_MS = 25_000;
/** A graph no tab polled for this long leaves the wait. */
export const INTEREST_MS = 60_000;
/** Events kept for the tabs (the daemon's own ring holds 256). */
export const LOG_MAX = 256;
const BACKOFF_MIN_MS = 1_000;
const BACKOFF_MAX_MS = 10_000;

/** Where a tab is: the daemon epoch and the hub generation it last saw, the last seq it has, and, for each open graph
 * whose changes it follows, the revision it applied there (a graph left out is not followed). */
export type Cursor = { epoch: string | null; gen: number; since: number; live?: ReadonlyMap<string, LiveCursor> };

/**
 * What a tab gets, for every graph it has open (plan 133). `reset`: rebuild agent state from `events` (every kept event
 * for those graphs). That happens on a new daemon epoch, a new hub generation (the hub rebuilt what it keeps, as when
 * a tab opens another graph), or a cursor older than what is kept. `changes`: what changed in each followed graph
 * since the revision the tab gave, by alias; a graph with nothing new is left out.
 */
export type EventsReply = { epoch: string; gen: number; seq: number; reset: boolean; events: AgentEvent[]; changes: Record<string, TabChange> };

export type HubDeps = {
  wait: (args: WaitArgs, signal: AbortSignal) => Promise<Waited>;
  /** Refuses a graph the picker would not open (lib/snapshot graphIds): agents never widen what the view shows. */
  check: (graph: string) => Promise<void>;
  now: () => number;
  sleep: (ms: number, signal?: AbortSignal) => Promise<void>;
};

const asError = (e: unknown) =>
  e instanceof UmxError ? e : new UmxError("ui_events_failed", "The view could not read agent events; see its terminal.");

export class Hub {
  private interest = new Map<string, number>();
  /** The graphs the kept events are complete for. */
  private covered = new Set<string>();
  private epoch: string | null = null;
  private gen = 0;
  private seq = 0;
  /** A cursor below this has lost events the hub dropped. */
  private floor = 0;
  private log: AgentEvent[] = [];
  private error: UmxError | null = null;
  private revisions = new Revisions();
  private listeners = new Set<() => void>();
  private running = false;
  private current: AbortController | null = null;

  constructor(private readonly deps: HubDeps) {}

  /** The answer for a tab with `graphs` open, held up to `holdMs` while there is nothing new. */
  async poll(graphs: readonly string[], cursor: Cursor, holdMs: number, signal?: AbortSignal): Promise<EventsReply> {
    for (const graph of graphs) await this.want(graph);
    const deadline = this.deps.now() + holdMs;
    const covered = () => graphs.every((g) => this.covered.has(g));
    for (;;) {
      if (this.error && !covered()) throw this.error;
      const answer = this.answer(graphs, cursor);
      if (answer) return answer;
      const left = deadline - this.deps.now();
      if (left <= 0 || signal?.aborted) {
        if (this.epoch !== null && covered()) return this.reply(false, [], {});
        if (this.error) throw this.error;
        throw new UmxError("ui_events_pending", "The view is still connecting to the umx daemon.", null, 503);
      }
      await this.changed(left, signal);
    }
  }

  /**
   * The graph's revision once this loop follows it, for a snapshot read next: what the snapshot holds is at least that
   * revision, so a tab that starts from it misses nothing (a change it already holds is applied again, harmlessly).
   * Null when the daemon has not answered within `holdMs`: the tab then reloads on its first overflow.
   */
  async follow(graph: string, holdMs: number, signal?: AbortSignal): Promise<LiveCursor | null> {
    // A daemon on another protocol refuses every wait: no revision is coming, so a snapshot does not wait for one.
    if (this.error?.code === "protocol_mismatch" && !this.covered.has(graph)) {
      void this.want(graph).catch(() => {});
      return null;
    }
    await this.want(graph);
    const deadline = this.deps.now() + holdMs;
    for (;;) {
      if (this.epoch !== null && this.covered.has(graph) && this.revisions.has(graph)) {
        return { epoch: this.epoch, rev: this.revisions.rev(graph) };
      }
      const left = deadline - this.deps.now();
      if (left <= 0 || signal?.aborted || (this.error && !this.covered.has(graph))) return null;
      await this.changed(left, signal);
    }
  }

  /** Marks `graph` as wanted by a tab now; a graph the loop does not cover yet makes it ask again at once. */
  private async want(graph: string) {
    await this.deps.check(graph);
    const isNew = !this.interest.has(graph);
    this.interest.set(graph, this.deps.now());
    if (isNew && this.running && !this.covered.has(graph)) this.current?.abort();
    if (!this.running) void this.run();
  }

  private reply(reset: boolean, events: AgentEvent[], changes: Record<string, TabChange>): EventsReply {
    return { epoch: this.epoch ?? "", gen: this.gen, seq: this.seq, reset, events, changes };
  }

  private answer(graphs: readonly string[], cursor: Cursor): EventsReply | null {
    const epoch = this.epoch;
    if (epoch === null || !graphs.every((g) => this.covered.has(g))) return null;
    const changes: Record<string, TabChange> = {};
    for (const g of graphs) {
      const at = cursor.live?.get(g);
      const change = at ? this.revisions.since(g, epoch, at) : null;
      if (change) changes[g] = change;
    }
    const open = new Set(graphs);
    // An open (ws009 M1b1) names graphs the tab may not have open yet: every tab gets it.
    const mine = (e: AgentEvent) => open.has(e.graph) || e.command.kind === "open";
    const reset = cursor.epoch !== epoch || cursor.gen !== this.gen || cursor.since < this.floor || cursor.since > this.seq;
    if (reset) return this.reply(true, this.log.filter(mine), changes);
    const events = this.log.filter((e) => mine(e) && e.seq > cursor.since);
    return events.length > 0 || Object.keys(changes).length > 0 ? this.reply(false, events, changes) : null;
  }

  private changed(ms: number, signal?: AbortSignal): Promise<void> {
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(timer);
        this.listeners.delete(done);
        signal?.removeEventListener("abort", done);
        resolve();
      };
      const timer = setTimeout(done, ms);
      this.listeners.add(done);
      signal?.addEventListener("abort", done, { once: true });
    });
  }

  private notify() {
    for (const l of [...this.listeners]) l();
  }

  private async run() {
    this.running = true;
    let backoff = 0;
    try {
      for (;;) {
        const now = this.deps.now();
        for (const [g, at] of this.interest) if (now - at > INTEREST_MS) this.interest.delete(g);
        const graphs = [...this.interest.keys()].sort();
        if (graphs.length === 0) {
          // Nothing kept is followed any more: the next tab starts afresh.
          this.covered = new Set();
          this.revisions.clear();
          break;
        }
        // A graph joining needs every kept event for it: ask from seq 0. The epoch is kept, so the revisions of the
        // graphs already followed stay good (the daemon reads revs given under another epoch as 0, an overflow).
        const joining = graphs.some((g) => !this.covered.has(g));
        const followed = new Set(graphs);
        this.revisions.keep(followed);
        for (const g of graphs) this.revisions.follow(g);
        const revs = Object.fromEntries(graphs.map((g) => [g, this.revisions.rev(g)]));
        const ctrl = new AbortController();
        this.current = ctrl;
        try {
          const w = await this.deps.wait(
            { epoch: this.epoch, since: joining ? 0 : this.seq, timeoutMs: WAIT_MS, graphs, revs },
            ctrl.signal,
          );
          this.error = null;
          backoff = 0;
          this.covered = followed;
          if (w.epoch !== this.epoch) {
            // Another daemon: every revision followed was the old one's; it answered each graph afresh.
            this.revisions.clear();
            for (const g of graphs) this.revisions.follow(g);
          }
          for (const c of w.changes) this.revisions.record(c);
          if (joining || w.reset || w.epoch !== this.epoch) {
            this.epoch = w.epoch;
            this.gen++;
            this.log = w.events.slice(-LOG_MAX);
            this.floor = 0;
          } else {
            this.log = this.log.filter((e) => this.covered.has(e.graph) || e.command.kind === "open").concat(w.events);
            const over = this.log.length - LOG_MAX;
            if (over > 0) {
              this.floor = this.log[over - 1].seq;
              this.log = this.log.slice(over);
            }
          }
          this.seq = w.seq;
        } catch (e) {
          if (ctrl.signal.aborted) continue;
          this.error = asError(e);
          if (!(e instanceof UmxError)) console.error(`umx ui events: ${String(e).slice(0, 200)}`);
          this.covered = new Set();
          this.notify();
          backoff = Math.min(Math.max(BACKOFF_MIN_MS, backoff * 2), BACKOFF_MAX_MS);
          await this.deps.sleep(backoff);
          continue;
        } finally {
          this.current = null;
        }
        this.notify();
      }
    } finally {
      this.running = false;
    }
  }
}

const EPOCH = /^[0-9a-f]{16}$/;
/** One followed graph in the query: `live=<alias>:<epoch or empty>:<rev>`. */
const LIVE = /^([a-z][a-z0-9_-]{0,63}):([0-9a-f]{16})?:(\d{1,15})$/;

/** A tab's cursor from the query string; anything malformed starts it afresh (a reset, or an overflow for its
 * graph's changes), never an error. A graph with no `live` entry is not followed. */
export function parseCursor(params: URLSearchParams): Cursor {
  const epoch = params.get("epoch");
  const gen = Number(params.get("gen"));
  const since = Number(params.get("since"));
  const live = new Map<string, LiveCursor>();
  for (const entry of params.getAll("live")) {
    const m = LIVE.exec(entry);
    const g = m?.[1] ?? /^([a-z][a-z0-9_-]{0,63}):/.exec(entry)?.[1];
    if (!g) continue;
    const rev = Number(m?.[3]);
    live.set(g, m && m[2] !== undefined && Number.isSafeInteger(rev) ? { epoch: m[2], rev } : { epoch: null, rev: 0 });
  }
  return {
    epoch: epoch !== null && EPOCH.test(epoch) ? epoch : null,
    gen: Number.isSafeInteger(gen) && gen >= 0 ? gen : -1,
    since: Number.isSafeInteger(since) && since >= 0 ? since : 0,
    live,
  };
}

/** How long a graph that passed the picker's check is trusted before it is checked again. */
export const CHECK_MS = 10_000;

/** Caches successful checks for CHECK_MS, so a busy agent does not run `umx graph list --all` per event. */
export function cachedCheck(check: (graph: string) => Promise<void>, now: () => number): (graph: string) => Promise<void> {
  const passed = new Map<string, number>();
  return async (graph) => {
    const at = passed.get(graph);
    if (at !== undefined && now() - at < CHECK_MS) return;
    passed.delete(graph);
    await check(graph);
    passed.set(graph, now());
  };
}

const HUB = Symbol.for("umx.ui.events");

/**
 * One value per process under `key`: the promise is stored before anything is awaited, so concurrent first callers
 * all get the same one (review R224-1). A failed `make` is forgotten, so the next caller tries again.
 */
export function processWide<T>(key: symbol, make: () => Promise<T>): Promise<T> {
  const store = globalThis as Record<symbol, Promise<T> | undefined>;
  const made = store[key];
  if (made) return made;
  const p = make();
  store[key] = p;
  p.catch(() => {
    if (store[key] === p) delete store[key];
  });
  return p;
}

/** The process-wide hub, on the real daemon: ONE UiWait loop for every tab. */
export function sharedHub(): Promise<Hub> {
  return processWide(HUB, async () => {
    const [{ umxSocket }, { graphIds }, { umx }] = await Promise.all([import("@/lib/umx/socket"), import("@/lib/snapshot"), import("@/lib/umx")]);
    const now = () => Date.now();
    return new Hub({
      wait: (args, signal) => umxSocket().wait(args, signal),
      check: cachedCheck(async (graph) => void (await graphIds(umx(), graph)), now),
      now,
      sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
    });
  });
}

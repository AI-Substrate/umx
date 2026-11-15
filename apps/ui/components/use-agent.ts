"use client";
// The browser half of the agent channel (plan 114): long-polls /api/events for the graphs on screen (one poll for all
// of them, plan 133), re-polls at once on every reply and backs off only on error. A hidden tab keeps polling; the
// app holds its camera moves until it is visible. Events are acted on once each (lib/agent receive), a reset rebuilds
// the state. The same poll carries each graph's live changes (plan 127): each is applied once, in revision order,
// before the next poll (lib/live plan).
import { type RefObject, useCallback, useEffect, useRef, useState } from "react";
import { type AgentState, type Applied, dropRemovedNodes, NO_AGENT, receive } from "@/lib/agent";
import { api, asIssue } from "@/lib/api";
import { setKey } from "@/lib/graph/keys";
import type { EventsReply } from "@/lib/events";
import { type Held, heldAt, heldChange, heldReceive, heldThen, owes } from "@/lib/hold";
import { type LiveCursor, type TabChange } from "@/lib/live";
import type { ApiError } from "@/lib/types";
import type { AgentEvent } from "@/lib/umx/socket";

const BACKOFF_MIN_MS = 1_000;
const BACKOFF_MAX_MS = 15_000;

export type AgentFeed = {
  state: AgentState;
  /** Events to act on (filters, camera), in order; `n` changes on every batch. */
  batch: { n: number; events: AgentEvent[] } | null;
  /** The last error, until a poll succeeds again. */
  error: ApiError | null;
};

/** A feed and the open graphs it belongs to (lib/graph/keys setKey). */
export type OwnedFeed = AgentFeed & { alias: string | null };

export const EMPTY_FEED: AgentFeed = { state: NO_AGENT, batch: null, error: null };

/** A reply for `alias`: its state, and its batch (or, with none new, the batch it already had; never another graph's). */
export function feedReplied(f: OwnedFeed, alias: string, state: AgentState, batch: AgentFeed["batch"]): OwnedFeed {
  return { alias, state, batch: batch ?? (f.alias === alias ? f.batch : null), error: null };
}

/** A failed poll for `alias`: the graph keeps its own state; another graph's state and batch never carry over (review
 * R224-2), so a graph whose first poll fails has no agent state until its own reply. */
export function feedFailed(f: OwnedFeed, alias: string, error: ApiError): OwnedFeed {
  return f.alias === alias ? { ...f, error } : { alias, ...EMPTY_FEED, error };
}

/** What the view may use for `alias`: its own feed, else nothing. */
export function feedFor(f: OwnedFeed, alias: string | null): AgentFeed {
  return f.alias === alias ? f : EMPTY_FEED;
}

/** Waits `ms`, or until `signal` aborts; its abort listener comes off when the wait ends either way, as the run's
 * signal outlives every backoff (review M1d-N3). */
export const sleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve) => {
    const stop = () => {
      clearTimeout(t);
      resolve();
    };
    const t = setTimeout(() => {
      signal.removeEventListener("abort", stop);
      resolve();
    }, ms);
    signal.addEventListener("abort", stop, { once: true });
  });

/** How the graphs on screen take live changes (GraphApp): where each starts, and how it applies a change or reloads. */
export type LiveHooks = {
  /** The revision of `alias`'s snapshot on screen when the poll starts. */
  start: (alias: string) => LiveCursor;
  /** Applies `change` to `alias` ("apply") or reloads that graph whole ("reload"). Resolves with the revision now on
   * screen and the node keys it took off; rejects to have the same change offered again on a later poll. */
  apply: (alias: string, change: TabChange, epoch: string, how: "apply" | "reload", signal: AbortSignal) => Promise<{ cursor: LiveCursor; removed: string[] }>;
  /** Hold (plan 152): changes are kept, not applied, until the person applies them (HoldControl). */
  held?: () => boolean;
};

/**
 * The person's control over held changes (plan 152). `apply`: the named graphs' held changes now, each through the one
 * apply path, in order (an explicit Apply, which Hold allows). `drain`: the same as automatic work, for Hold to Live.
 * `hold`: entering Hold ends every automatic apply (Live's own and a drain): one not yet committed commits nothing and
 * its changes stay held, and no further graph starts (review B1). Each resolves true when every graph applied.
 */
export type HoldControl = { apply: (aliases: readonly string[]) => Promise<boolean>; drain: (aliases: readonly string[]) => Promise<boolean>; hold: () => void };

/** A signal that aborts when either does, and `release`, which takes its listeners off both again: the run's signals
 * live long, so each automatic apply releases its own once it settles (review M1d-N3). */
export function either(a: AbortSignal, b: AbortSignal): { signal: AbortSignal; release: () => void } {
  if (a.aborted || b.aborted) return { signal: AbortSignal.abort(), release: () => {} };
  const c = new AbortController();
  const stop = () => c.abort();
  a.addEventListener("abort", stop, { once: true });
  b.addEventListener("abort", stop, { once: true });
  return {
    signal: c.signal,
    release: () => {
      a.removeEventListener("abort", stop);
      b.removeEventListener("abort", stop);
    },
  };
}

/** The query for one poll: every open graph, the agent cursor, and the revision each graph on screen is at. */
export function pollQuery(aliases: readonly string[], cursor: { epoch: string; gen: number; since: number }, live: ReadonlyMap<string, LiveCursor>): URLSearchParams {
  const q = new URLSearchParams();
  for (const a of aliases) q.append("graph", a);
  q.set("epoch", cursor.epoch);
  q.set("gen", String(cursor.gen));
  q.set("since", String(cursor.since));
  for (const a of aliases) {
    const at = live.get(a);
    if (at) q.append("live", `${a}:${at.epoch ?? ""}:${at.rev}`);
  }
  return q;
}

/** What the poll loop needs from outside: the request, a sleep, and where its feed goes. Injected for tests. */
export type FollowDeps = {
  get: (path: string, signal: AbortSignal) => Promise<EventsReply>;
  sleep: (ms: number, signal: AbortSignal) => Promise<void>;
  publish: (update: (f: OwnedFeed) => OwnedFeed) => void;
  /** Each open graph's held changes, after every change to them (plan 152). */
  held?: (held: ReadonlyMap<string, Held>) => void;
  /** The run's HoldControl while it runs; (null, it) once it stops. */
  control?: (c: HoldControl | null, was?: HoldControl) => void;
};

/**
 * The tab's one poll for the open graphs `aliases`, until `signal` aborts: agent events go to the feed; each graph's
 * change is applied through `hooks()` before the next poll, and only when it starts at the revision on screen for
 * that graph (lib/live plan), so a duplicate or out-of-order reply is never applied twice and no revision is skipped.
 * Once `signal` aborts (the tab opened other graphs) nothing more from this run is processed, applied or published: a
 * reply or apply that settles late is dropped, and an update already queued leaves the newer feed as it is (review
 * R261-1).
 */
export async function follow(aliases: readonly string[], hooks: () => LiveHooks | null, signal: AbortSignal, deps: FollowDeps) {
  const key = setKey(aliases);
  const publish = (update: (f: OwnedFeed) => OwnedFeed) => deps.publish((f) => (signal.aborted ? f : update(f)));
  let cursor = { epoch: "", gen: -1, since: 0 };
  let applied: Applied = null;
  let state = NO_AGENT;
  let n = 0;
  let backoff = 0;
  // Each graph's changes told and not applied (lib/hold): Live applies them at once, Hold keeps them (plan 152).
  const held = new Map<string, Held>();
  const first = hooks();
  for (const a of aliases) held.set(a, heldAt(first?.start(a) ?? { epoch: null, rev: 0 }));
  // A graph's held changes while their apply is under way: still shown as held until it commits (review N1).
  const inFlight = new Map<string, Held>();
  const showHeld = () => {
    if (signal.aborted) return;
    const shown = new Map<string, Held>();
    for (const [a, h] of held) {
      const going = inFlight.get(a);
      shown.set(a, going ? heldThen(going, h) : h);
    }
    deps.held?.(shown);
  };
  // Automatic work (Live's applies, Hold to Live's drain) runs under this; Hold aborts it and starts a new one.
  let auto = new AbortController();
  showHeld();
  const later = async () => {
    backoff = Math.min(Math.max(BACKOFF_MIN_MS, backoff * 2), BACKOFF_MAX_MS);
    await deps.sleep(backoff, signal);
  };
  // One apply at a time, the loop's and the person's alike, so no two ever run on one graph.
  let chain: Promise<unknown> = Promise.resolve();
  const serial = <T,>(run: () => Promise<T>): Promise<T> => {
    const p = chain.then(run);
    chain = p.catch(() => {});
    return p;
  };
  // A graph's held changes through the one apply path. Changes told while it runs are kept from where it ends; if it
  // fails, what it took is owed again ahead of them, and the next poll offers it again.
  // `automatic`: the signal Hold aborts, for automatic work; the apply path rechecks it after every await, before it
  // changes the canvas, so an apply Hold overtook commits nothing and its changes are held again.
  const applyHeld = (alias: string, h: LiveHooks, automatic?: AbortSignal) =>
    serial(async (): Promise<{ ok: boolean; removed: string[] }> => {
      const owed = held.get(alias);
      const next = owed && !signal.aborted && !automatic?.aborted ? heldChange(owed) : null;
      if (!owed || !next) return { ok: !automatic?.aborted, removed: [] };
      held.set(alias, heldAt(owed.seen));
      inFlight.set(alias, owed);
      showHeld();
      const both = automatic ? either(signal, automatic) : null;
      try {
        const done = await h.apply(alias, next.change, next.epoch, next.how, both?.signal ?? signal);
        inFlight.delete(alias);
        showHeld();
        return { ok: !signal.aborted, removed: done.removed };
      } catch {
        inFlight.delete(alias);
        if (!signal.aborted) {
          held.set(alias, heldThen(owed, held.get(alias) ?? heldAt(owed.seen)));
          showHeld();
        }
        return { ok: false, removed: [] };
      } finally {
        both?.release();
      }
    });
  // Nodes taken off leave the agent's highlight and focus.
  const prune = (removed: ReadonlySet<string>) => {
    if (removed.size === 0 || signal.aborted) return;
    state = dropRemovedNodes(state, removed);
    const pruned = state;
    publish((f) => feedReplied(f, key, pruned, null));
  };
  const applyList = async (list: readonly string[], automatic?: AbortSignal) => {
    const h = hooks();
    if (!h || signal.aborted) return false;
    const removed = new Set<string>();
    let ok = true;
    for (const alias of list) {
      if (!aliases.includes(alias)) continue;
      if (automatic?.aborted) {
        ok = false;
        break;
      }
      const r = await applyHeld(alias, h, automatic);
      if (signal.aborted) return false;
      ok &&= r.ok;
      for (const id of r.removed) removed.add(id);
    }
    prune(removed);
    return ok;
  };
  const control: HoldControl = {
    apply: (list) => applyList(list),
    drain: (list) => applyList(list, auto.signal),
    hold: () => {
      auto.abort();
      auto = new AbortController();
    },
  };
  deps.control?.(control);
  signal.addEventListener("abort", () => deps.control?.(null, control), { once: true });
  while (!signal.aborted) {
    const h = hooks();
    const holding = h?.held?.() ?? false;
    // What each graph asks from: the last revision told, so the poll waits for new ones; but in Live with changes
    // still owed (an apply failed), the revision on screen, so the poll answers at once and the apply is tried again.
    const asked = new Map<string, LiveCursor>();
    for (const [a, o] of held) asked.set(a, !holding && owes(o) ? o.from : o.seen);
    let reply: EventsReply;
    try {
      reply = await deps.get(`/api/events?${pollQuery(aliases, cursor, asked)}`, signal);
    } catch (e) {
      if (signal.aborted) break;
      const error = asIssue(e);
      publish((f) => feedFailed(f, key, error));
      await later();
      continue;
    }
    if (signal.aborted) break;
    const got = receive(state, applied, reply);
    state = got.state;
    applied = got.applied;
    cursor = { epoch: reply.epoch, gen: reply.gen, since: reply.seq };
    backoff = 0;
    const batch = got.act.length > 0 ? { n: ++n, events: got.act } : null;
    const now = state;
    publish((f) => feedReplied(f, key, now, batch));
    for (const alias of aliases) {
      const change = reply.changes?.[alias];
      const o = held.get(alias);
      if (change && o) held.set(alias, heldReceive(o, reply.epoch, change));
    }
    showHeld();
    const hNow = hooks();
    if (!hNow || hNow.held?.()) continue;
    const automatic = auto.signal;
    let failed = false;
    const removed = new Set<string>();
    for (const alias of aliases) {
      // Hold came in meanwhile: what is left stays held.
      if (automatic.aborted) break;
      const r = await applyHeld(alias, hNow, automatic);
      if (signal.aborted) break;
      failed ||= !r.ok && !automatic.aborted;
      for (const id of r.removed) removed.add(id);
    }
    if (signal.aborted) break;
    prune(removed);
    if (failed) await later();
  }
}

const NO_HELD: ReadonlyMap<string, Held> = new Map();

export function useAgent(
  aliases: readonly string[] | null,
  hooks?: RefObject<LiveHooks | null>,
): AgentFeed & {
  held: ReadonlyMap<string, Held>;
  applyHeld: (aliases: readonly string[]) => Promise<boolean>;
  drainHeld: (aliases: readonly string[]) => Promise<boolean>;
  holdNow: () => void;
} {
  const key = aliases && aliases.length > 0 ? setKey(aliases) : null;
  const [feed, setFeed] = useState<OwnedFeed>({ alias: key, ...EMPTY_FEED });
  const [held, setHeld] = useState<{ key: string | null; held: ReadonlyMap<string, Held> }>({ key, held: NO_HELD });
  const control = useRef<HoldControl | null>(null);

  useEffect(() => {
    if (!key) return;
    const ctrl = new AbortController();
    void follow(key.split(","), () => hooks?.current ?? null, ctrl.signal, {
      get: (path, signal) => api<EventsReply>(path, signal),
      sleep,
      publish: setFeed,
      held: (h) => setHeld({ key, held: h }),
      control: (c, was) => {
        if (c) control.current = c;
        else if (control.current === was) control.current = null;
      },
    });
    return () => ctrl.abort();
  }, [key, hooks]);
  const applyHeld = useCallback((list: readonly string[]) => control.current?.apply(list) ?? Promise.resolve(false), []);
  const drainHeld = useCallback((list: readonly string[]) => control.current?.drain(list) ?? Promise.resolve(false), []);
  const holdNow = useCallback(() => control.current?.hold(), []);

  // A feed for another set of graphs is never shown for this one.
  return { ...feedFor(feed, key), held: held.key === key ? held.held : NO_HELD, applyHeld, drainHeld, holdNow };
}

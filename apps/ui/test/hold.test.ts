// Live and Hold (lib/hold.ts, components/use-agent follow; plan 152): what is held per graph, how it is counted, and
// that applying it goes through the one apply path, one graph at a time.
import { describe, expect, it, vi } from "vitest";
import { follow, type HoldControl, type LiveHooks, type OwnedFeed, EMPTY_FEED, sleep } from "@/components/use-agent";
import { NO_AGENT } from "@/lib/agent";
import type { EventsReply } from "@/lib/events";
import { qid } from "@/lib/graph/keys";
import { countText, type Held, heldAt, heldChange, heldCount, heldMayHave, heldReceive, heldThen, heldTotal, liveMode, namedHeld, noteFor } from "@/lib/hold";
import { MAX_CHANGE_IDS, type TabChange } from "@/lib/live";
import type { AgentEvent } from "@/lib/umx/socket";

const E1 = "00000000000000e1";
const E2 = "00000000000000e2";
const NONE = { added: [] as string[], edited: [] as string[], removed: [] as string[] };
const changed = (from: number, to: number, nodes = NONE): TabChange => ({ kind: "changed", from, to, nodes, edges: NONE });
const N = (c: string) => `01${c.repeat(24)}`;

describe("what a graph holds, and how it is counted", () => {
  const at = { epoch: E1, rev: 4 };

  it("counts every revision, one that touched no id too, and merges the ids", () => {
    let h = heldAt(at);
    expect(heldCount(h)).toBe(0);
    h = heldReceive(h, E1, changed(4, 5, { ...NONE, added: [N("A")] }));
    h = heldReceive(h, E1, changed(5, 7));
    // A duplicate and an out-of-order answer are skipped (lib/live plan).
    h = heldReceive(h, E1, changed(5, 7));
    h = heldReceive(h, E1, changed(4, 6));
    expect(heldCount(h)).toBe(3);
    expect(heldChange(h)).toEqual({ change: changed(4, 7, { ...NONE, added: [N("A")] }), how: "apply", epoch: E1 });
    // Revisions with no ids are owed too: applying them still runs the change's read (the symmetry refresh).
    expect(heldChange(heldReceive(heldAt(at), E1, changed(4, 5)))).toEqual({ change: changed(4, 5), how: "apply", epoch: E1 });
    expect(heldChange(h)).not.toBeNull();
    expect(heldChange(heldAt(at))).toBeNull();
  });

  it("says many, never a number, after an overflow, a feed reset, too many ids, or before the revision is known", () => {
    const many = (h: Held) => [heldCount(h), heldChange(h)?.how];
    expect(many(heldReceive(heldAt(at), E1, { kind: "overflow", rev: 9 }))).toEqual([null, "reload"]);
    expect(many(heldReceive(heldAt(at), E2, { kind: "overflow", rev: 2 }))).toEqual([null, "reload"]);
    const lots = { ...NONE, added: Array.from({ length: MAX_CHANGE_IDS + 1 }, (_v, i) => `01${String(i).padStart(24, "0")}`) };
    expect(many(heldReceive(heldAt(at), E1, changed(4, 5, lots)))).toEqual([null, "reload"]);
    expect(heldCount(heldAt({ epoch: null, rev: 0 }))).toBeNull();
    expect(heldTotal(new Map([["alpha", heldReceive(heldAt(at), E1, changed(4, 6))], ["beta", heldAt(at)]]), ["alpha", "beta"])).toBe(2);
    expect(heldTotal(new Map([["alpha", heldReceive(heldAt(at), E1, changed(4, 6))], ["beta", heldAt({ epoch: null, rev: 0 })]]), ["alpha", "beta"])).toBeNull();
    expect([countText(null), countText(12)]).toEqual(["many", "12"]);
  });

  it("puts a failed apply's changes back ahead of the ones told meanwhile", () => {
    const a = heldReceive(heldAt(at), E1, changed(4, 5, { ...NONE, added: [N("A")] }));
    const later = heldReceive(heldAt(a.seen), E1, changed(5, 6, { ...NONE, removed: [N("A")] }));
    const both = heldThen(a, later);
    expect([both.from, both.seen, heldCount(both)]).toEqual([at, { epoch: E1, rev: 6 }, 2]);
    // Added then removed: the reader never had it.
    expect(both.touched?.nodes).toEqual(NONE);
  });

  it("knows which nodes may exist only in held changes", () => {
    const h = heldReceive(heldAt(at), E1, changed(4, 5, { ...NONE, added: [N("A")], edited: [N("B")] }));
    expect([heldMayHave(h, N("A")), heldMayHave(h, N("B")), heldMayHave(h, N("C")), heldMayHave(heldAt(at), N("A"))]).toEqual([true, true, false, false]);
    expect(heldMayHave(heldReceive(heldAt(at), E1, { kind: "overflow", rev: 9 }), N("C"))).toBe(true);
  });

  it("names the graph whose held changes an agent's command named a node in, using that graph's own held set", () => {
    const alphaHeld = heldReceive(heldAt(at), E1, changed(4, 5, { ...NONE, added: [N("A")] }));
    const held = new Map([["alpha", alphaHeld], ["beta", heldAt(at)]]);
    const onCanvas = (key: string) => key === qid("beta", N("B"));
    const hl = (graph: string, id: string) => ({ graph, command: { kind: "highlight", nodes: [qid(graph, id)] } });
    expect(namedHeld([hl("alpha", N("A"))], held, onCanvas)).toBe("alpha");
    // beta holds nothing: the same id named in beta is not a held node of beta's.
    expect(namedHeld([hl("beta", N("A"))], held, onCanvas)).toBeNull();
    expect(namedHeld([{ graph: "alpha", command: { kind: "focus", node: qid("alpha", N("A")) } }], held, onCanvas)).toBe("alpha");
    expect(namedHeld([hl("beta", N("B")), { graph: "alpha", command: { kind: "filter" } }], held, onCanvas)).toBeNull();
  });

  it("shows a hold note only for the open graphs it was said for, and its Apply only for an open graph (review B2)", () => {
    const both = { set: "alpha,beta", aliases: ["alpha", "beta"] };
    const betaOnly = { set: "beta", aliases: ["beta"] };
    expect(noteFor({ set: "alpha,beta", kind: "pending", alias: "alpha" }, both)).toEqual({ text: "agent named a node in pending changes (alpha)", apply: "alpha" });
    expect(noteFor({ set: "alpha,beta", kind: "pending", alias: "alpha" }, betaOnly)).toBeNull();
    expect(noteFor({ set: "alpha,beta", kind: "removed" }, betaOnly)).toBeNull();
    // A note for a graph not open is never shown, even under the same key.
    expect(noteFor({ set: "beta", kind: "pending", alias: "alpha" }, betaOnly)).toBeNull();
    expect(noteFor({ set: "beta", kind: "pending", alias: "beta" }, betaOnly)).toEqual({ text: "agent named a node in pending changes (beta)", apply: "beta" });
    expect(noteFor({ set: "beta", kind: "removed" }, betaOnly)).toEqual({ text: "your selected node was removed", apply: null });
  });

  it("keeps the mode as a name only", () => {
    expect([liveMode("hold"), liveMode("live"), liveMode('{"rev":4}'), liveMode(null)]).toEqual(["hold", "live", "live", "live"]);
  });
});

describe("the poll holds changes per graph and applies them only when asked (plan 152)", () => {
  type Run = {
    hold: { on: boolean };
    applied: { alias: string; change: TabChange; how: string }[];
    paths: string[];
    held: () => ReadonlyMap<string, Held>;
    feed: () => OwnedFeed;
    send: (r: EventsReply) => void;
    polled: (n: number) => Promise<void>;
    control: () => HoldControl;
    stop: () => void;
    gate: { block: boolean; release: () => void; failNext: boolean; inside: boolean };
    done: Promise<void>;
  };
  const reply = (changes: Record<string, TabChange>, events: AgentEvent[] = []): EventsReply => ({ epoch: E1, gen: 1, seq: events.length, reset: false, events, changes });

  /** Abort listeners now on `signal` (added and not yet removed). */
  function listening(signal: AbortSignal): Set<unknown> {
    const on = new Set<unknown>();
    const [add, remove] = [signal.addEventListener.bind(signal), signal.removeEventListener.bind(signal)];
    signal.addEventListener = ((t: string, f: EventListenerOrEventListenerObject, o?: AddEventListenerOptions | boolean) => {
      if (t === "abort") on.add(f);
      add(t, f, o);
    }) as typeof signal.addEventListener;
    signal.removeEventListener = ((t: string, f: EventListenerOrEventListenerObject, o?: EventListenerOptions | boolean) => {
      if (t === "abort") on.delete(f);
      remove(t, f, o);
    }) as typeof signal.removeEventListener;
    return on;
  }

  function start(removed: string[] = [], watch?: (signal: AbortSignal) => void): Run {
    const ctrl = new AbortController();
    watch?.(ctrl.signal);
    const queue: EventsReply[] = [];
    let wake: (() => void) | null = null;
    const paths: string[] = [];
    const waiters: { n: number; go: () => void }[] = [];
    let held: ReadonlyMap<string, Held> = new Map();
    let feed: OwnedFeed = { alias: "alpha,beta", ...EMPTY_FEED };
    let control: HoldControl | null = null;
    const applied: Run["applied"] = [];
    const gate: Run["gate"] = { block: false, release: () => {}, failNext: false, inside: false };
    const hold = { on: true };
    const hooks: LiveHooks = {
      start: (a) => ({ epoch: E1, rev: a === "alpha" ? 4 : 10 }),
      held: () => hold.on,
      // As GraphApp applyChange: after its read, a stopped signal means nothing is committed.
      apply: async (alias, change, epoch, how, signal) => {
        if (gate.block) {
          gate.inside = true;
          await new Promise<void>((r) => (gate.release = r));
          gate.inside = false;
        }
        if (signal.aborted) throw new Error("the graphs on screen changed");
        if (gate.failNext) {
          gate.failNext = false;
          throw new Error("rows failed");
        }
        applied.push({ alias, change, how });
        return { cursor: { epoch, rev: change.kind === "changed" ? change.to : change.rev }, removed };
      },
    };
    const done = follow(["alpha", "beta"], () => hooks, ctrl.signal, {
      get: async (path, signal) => {
        paths.push(path);
        for (const w of waiters.filter((w) => paths.length >= w.n)) w.go();
        while (queue.length === 0) {
          if (signal.aborted) throw new Error("aborted");
          // The listener comes off once the wait ends, so the test counts only follow's own.
          let off = () => {};
          await new Promise<void>((r) => {
            wake = r;
            const stop = () => r();
            signal.addEventListener("abort", stop, { once: true });
            off = () => signal.removeEventListener("abort", stop);
          });
          off();
        }
        return queue.shift()!;
      },
      sleep: async () => {},
      publish: (u) => {
        feed = u(feed);
      },
      held: (h) => {
        held = h;
      },
      control: (c) => {
        control = c;
      },
    });
    return {
      hold,
      applied,
      paths,
      held: () => held,
      feed: () => feed,
      send: (r) => {
        queue.push(r);
        wake?.();
      },
      polled: (n) => new Promise<void>((go) => (paths.length >= n ? go() : waiters.push({ n, go }))),
      control: () => control!,
      stop: () => ctrl.abort(),
      gate,
      done,
    };
  }
  const asked = (path: string) => new URLSearchParams(path.split("?")[1]).getAll("live");

  it("takes each automatic apply's abort listener off the run's signal once it settles (review M1d-N3)", async () => {
    let on = new Set<unknown>();
    const r = start([], (signal) => (on = listening(signal)));
    r.hold.on = false;
    const counts: number[] = [];
    for (let i = 0; i < 40; i++) {
      r.send(reply({ alpha: changed(4 + i, 5 + i) }));
      await r.polled(i + 2);
      counts.push(on.size);
    }
    expect(r.applied).toHaveLength(40);
    // Flat, whatever the number of applies: no closure is left behind on the long-lived signal.
    expect(counts[39]).toBe(counts[1]);
    r.stop();
    await r.done;
  });

  it("takes a backoff's abort listener off once it has waited", async () => {
    const ctrl = new AbortController();
    const on = listening(ctrl.signal);
    for (let i = 0; i < 5; i++) await sleep(0, ctrl.signal);
    expect(on.size).toBe(0);
    const late = sleep(60_000, ctrl.signal);
    ctrl.abort();
    await late;
  });

  it("counts each graph's changes, applies nothing, and polls from the last revision told so it waits", async () => {
    const r = start();
    r.send(reply({ alpha: changed(4, 6), beta: changed(10, 11) }));
    await r.polled(2);
    r.send(reply({ alpha: changed(6, 7) }));
    await r.polled(3);
    expect(r.applied).toEqual([]);
    expect([heldCount(r.held().get("alpha")), heldCount(r.held().get("beta"))]).toEqual([3, 1]);
    expect(asked(r.paths[2])).toEqual([`alpha:${E1}:7`, `beta:${E1}:11`]);
    r.stop();
    await r.done;
  });

  it("applies one graph through the one apply path and leaves the other's held changes as they are; all is each", async () => {
    const r = start();
    r.send(reply({ alpha: changed(4, 6, { ...NONE, added: [N("A")] }), beta: changed(10, 12) }));
    await r.polled(2);
    expect(await r.control().apply(["alpha"])).toBe(true);
    expect(r.applied).toEqual([{ alias: "alpha", change: changed(4, 6, { ...NONE, added: [N("A")] }), how: "apply" }]);
    expect([heldCount(r.held().get("alpha")), heldCount(r.held().get("beta"))]).toEqual([0, 2]);
    r.send(reply({ beta: { kind: "overflow", rev: 30 } }));
    await r.polled(3);
    expect(heldCount(r.held().get("beta"))).toBeNull();
    await r.control().apply(["alpha", "beta"]);
    expect(r.applied.map((a) => [a.alias, a.how])).toEqual([["alpha", "apply"], ["beta", "reload"]]);
    expect(r.held().get("beta")).toEqual(heldAt({ epoch: E1, rev: 30 }));
    r.stop();
    await r.done;
  });

  it("Hold to Live applies everything held, then applies each change as it comes", async () => {
    const r = start();
    r.send(reply({ alpha: changed(4, 5), beta: changed(10, 11) }));
    await r.polled(2);
    r.hold.on = false;
    await r.control().apply(["alpha", "beta"]);
    r.send(reply({ alpha: changed(5, 6) }));
    await r.polled(3);
    expect(r.applied.map((a) => [a.alias, a.change.kind === "changed" ? `${a.change.from}-${a.change.to}` : "reload"])).toEqual([["alpha", "4-5"], ["beta", "10-11"], ["alpha", "5-6"]]);
    r.stop();
    await r.done;
  });

  it("an apply that fails owes its changes again ahead of the ones told meanwhile", async () => {
    const r = start();
    r.send(reply({ alpha: changed(4, 5) }));
    await r.polled(2);
    r.gate.block = true;
    r.gate.failNext = true;
    const p = r.control().apply(["alpha"]);
    await Promise.resolve();
    r.send(reply({ alpha: changed(5, 6) }));
    await r.polled(3);
    r.gate.release();
    expect(await p).toBe(false);
    const h = r.held().get("alpha")!;
    expect([h.from.rev, h.seen.rev, heldCount(h)]).toEqual([4, 6, 2]);
    r.stop();
    await r.done;
  });

  it("an apply cancelled mid-way (the open graphs changed) publishes and prunes nothing", async () => {
    const r = start([qid("alpha", N("A"))]);
    r.send(reply({ alpha: changed(4, 5), beta: changed(10, 11) }, [{ seq: 1, at_ms: 1, graph: "alpha", by: null, command: { kind: "highlight", nodes: [N("A")], add: false } }]));
    await r.polled(2);
    r.gate.block = true;
    const p = r.control().apply(["alpha", "beta"]);
    await Promise.resolve();
    const heldBefore = r.held();
    r.stop();
    r.gate.release();
    expect(await p).toBe(false);
    await r.done;
    // alpha's read settled after the stop, so it commits nothing; beta never starts, and nothing is published.
    expect(r.applied.map((a) => a.alias)).toEqual([]);
    expect(r.held()).toBe(heldBefore);
    expect(r.feed().state.highlight).toEqual(new Set([qid("alpha", N("A"))]));
    expect(r.feed().state).not.toBe(NO_AGENT);
  });

  it("Hold ends a Live apply under way: it commits nothing, the next graph does not start, and both stay held (review B1)", async () => {
    const r = start();
    r.hold.on = false;
    r.gate.block = true;
    r.send(reply({ alpha: changed(4, 5), beta: changed(10, 11) }));
    await vi.waitFor(() => expect(r.gate.inside).toBe(true));
    // While alpha's read is out, its changes still count as held (review N1).
    expect(heldCount(r.held().get("alpha"))).toBe(1);
    r.hold.on = true;
    r.control().hold();
    r.gate.release();
    await r.polled(2);
    expect(r.applied).toEqual([]);
    expect([heldCount(r.held().get("alpha")), heldCount(r.held().get("beta"))]).toEqual([1, 1]);
    // An explicit Apply while held still applies.
    r.gate.block = false;
    expect(await r.control().apply(["alpha"])).toBe(true);
    expect(r.applied.map((a) => a.alias)).toEqual(["alpha"]);
    r.stop();
    await r.done;
  });

  it("Hold, Live, Hold again while the drain is under way: the drain stops and what it had not committed stays held", async () => {
    const r = start();
    r.send(reply({ alpha: changed(4, 5), beta: changed(10, 11) }));
    await r.polled(2);
    r.hold.on = false;
    r.gate.block = true;
    const drained = r.control().drain(["alpha", "beta"]);
    await vi.waitFor(() => expect(r.gate.inside).toBe(true));
    r.hold.on = true;
    r.control().hold();
    r.gate.release();
    expect(await drained).toBe(false);
    expect(r.applied).toEqual([]);
    expect([heldCount(r.held().get("alpha")), heldCount(r.held().get("beta"))]).toEqual([1, 1]);
    // Live again: a new drain applies both.
    r.hold.on = false;
    r.gate.block = false;
    expect(await r.control().drain(["alpha", "beta"])).toBe(true);
    expect(r.applied.map((a) => a.alias)).toEqual(["alpha", "beta"]);
    r.stop();
    await r.done;
  });

  it("takes nodes an apply removed out of the agent's highlight", async () => {
    const r = start([qid("alpha", N("A"))]);
    r.send(reply({ alpha: changed(4, 5) }, [{ seq: 1, at_ms: 1, graph: "alpha", by: null, command: { kind: "highlight", nodes: [N("A")], add: false } }]));
    await r.polled(2);
    expect(r.feed().state.highlight.size).toBe(1);
    await r.control().apply(["alpha"]);
    expect(r.feed().state.highlight.size).toBe(0);
    r.stop();
    await r.done;
  });
});

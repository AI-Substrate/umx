// The server's one UiWait loop and the tabs it answers (lib/events.ts, plan 114). The daemon is scripted: each wait
// is held until the test answers it.
import { describe, expect, it, vi } from "vitest";
import { cachedCheck, type Cursor, Hub, LOG_MAX, parseCursor } from "@/lib/events";
import { UmxError } from "@/lib/umx";
import type { AgentEvent, Waited, WaitArgs } from "@/lib/umx/socket";

const E1 = "00000000000000e1";
const E2 = "00000000000000e2";
const N = "01NODEAAAAAAAAAAAAAAAAAAAA";
const ev = (seq: number, graph = "demo"): AgentEvent => ({ seq, at_ms: 1000 + seq, graph, by: null, command: { kind: "focus", node: N, hops: 1 } });
const flush = () => new Promise((r) => setTimeout(r, 5));

function daemon(check: (g: string) => Promise<void> = async () => {}) {
  type Answer = Omit<Waited, "changes"> & { changes?: Waited["changes"] };
  const calls: { args: WaitArgs; signal: AbortSignal; answer: (w: Answer) => void; fail: (e: unknown) => void }[] = [];
  const wait = vi.fn((args: WaitArgs, signal: AbortSignal) =>
    new Promise<Waited>((answer, fail) => {
      calls.push({ args, signal, answer: (w) => answer({ changes: [], ...w }), fail });
      signal.addEventListener("abort", () => fail(new Error("aborted")));
    }),
  );
  const hub = new Hub({ wait, check, now: () => Date.now(), sleep: async () => {} });
  const last = () => calls[calls.length - 1];
  return { hub, wait, calls, last };
}

const fresh: Cursor = { epoch: null, gen: -1, since: 0 };

describe("one daemon wait for every tab", () => {
  it("starts afresh, answers a reset with that graph's events only, then serves the next poll from the same loop", async () => {
    const d = daemon();
    const first = d.hub.poll(["demo"], fresh, 1000);
    await flush();
    expect(d.last().args).toEqual({ epoch: null, since: 0, timeoutMs: 25_000, graphs: ["demo"], revs: { demo: 0 } });
    d.last().answer({ epoch: E1, seq: 2, reset: true, events: [ev(1), ev(2, "other")] });
    const r1 = await first;
    expect(r1).toMatchObject({ epoch: E1, gen: 1, seq: 2, reset: true });
    expect(r1.events.map((e) => e.seq)).toEqual([1]);
    await flush();
    expect(d.last().args).toMatchObject({ epoch: E1, since: 2, graphs: ["demo"] });
    // Two tabs on the graph wait on the same daemon wait.
    const cursor = { epoch: r1.epoch, gen: r1.gen, since: r1.seq };
    const a = d.hub.poll(["demo"], cursor, 1000);
    const b = d.hub.poll(["demo"], cursor, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 3, reset: false, events: [ev(3)] });
    for (const r of await Promise.all([a, b])) expect(r).toMatchObject({ reset: false, seq: 3, events: [{ seq: 3 }] });
    expect(d.wait).toHaveBeenCalledTimes(3);
  });

  it("answers an empty reply when nothing happens within the hold", async () => {
    const d = daemon();
    const first = d.hub.poll(["demo"], fresh, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [] });
    const r = await first;
    expect(await d.hub.poll(["demo"], { epoch: E1, gen: r.gen, since: 0 }, 30)).toMatchObject({ reset: false, events: [] });
  });

  it("re-asks from seq 0 in the same epoch when a tab opens another graph, so every tab resets and rebuilds", async () => {
    const d = daemon();
    const p = d.hub.poll(["demo"], fresh, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 1, reset: true, events: [ev(1)] });
    const r = await p;
    await flush();
    const waiting = d.last();
    const other = d.hub.poll(["other"], fresh, 1000);
    await flush();
    expect(waiting.signal.aborted).toBe(true);
    expect(d.last().args).toMatchObject({ epoch: E1, since: 0, graphs: ["demo", "other"] });
    d.last().answer({ epoch: E1, seq: 2, reset: true, events: [ev(1), ev(2, "other")] });
    expect((await other).events.map((e) => e.seq)).toEqual([2]);
    const again = await d.hub.poll(["demo"], { epoch: r.epoch, gen: r.gen, since: r.seq }, 1000);
    expect(again).toMatchObject({ reset: true, gen: r.gen + 1, events: [{ seq: 1 }] });
  });

  it("resets a tab on a new daemon epoch and on a cursor older than what is kept", async () => {
    const d = daemon();
    const p = d.hub.poll(["demo"], fresh, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [] });
    const r = await p;
    await flush();
    const many = Array.from({ length: LOG_MAX + 10 }, (_v, i) => ev(i + 1));
    d.last().answer({ epoch: E1, seq: many.length, reset: false, events: many });
    await flush();
    expect(await d.hub.poll(["demo"], { epoch: E1, gen: r.gen, since: 5 }, 1000)).toMatchObject({ reset: true });
    expect(await d.hub.poll(["demo"], { epoch: E1, gen: r.gen, since: many.length - 1 }, 1000)).toMatchObject({ reset: false, events: [{ seq: many.length }] });
    d.last().answer({ epoch: E2, seq: 1, reset: true, events: [ev(1)] });
    await flush();
    expect(await d.hub.poll(["demo"], { epoch: E1, gen: r.gen, since: many.length }, 1000)).toMatchObject({ epoch: E2, reset: true });
  });

  it("passes a daemon failure to the tabs, then recovers", async () => {
    const d = daemon();
    const p = d.hub.poll(["demo"], fresh, 1000);
    await flush();
    d.last().fail(new UmxError("daemon_not_running", "down", "umx daemon start", 503));
    await expect(p).rejects.toMatchObject({ code: "daemon_not_running" });
    // While it is down a tab's poll fails at once (the browser backs off); the loop retries on its own.
    await expect(d.hub.poll(["demo"], fresh, 1000)).rejects.toMatchObject({ code: "daemon_not_running" });
    await flush();
    expect(d.wait).toHaveBeenCalledTimes(2);
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [] });
    await flush();
    await expect(d.hub.poll(["demo"], fresh, 1000)).resolves.toMatchObject({ reset: true });
  });

  it("never waits on a graph the picker would not open", async () => {
    const d = daemon(async (g) => {
      if (g === "vault") throw new UmxError("ui_graph_not_open", "no", "umx graph load", 409);
    });
    await expect(d.hub.poll(["vault"], fresh, 100)).rejects.toMatchObject({ code: "ui_graph_not_open" });
    expect(d.wait).not.toHaveBeenCalled();
  });
});

describe("cursor and checks", () => {
  it("reads a malformed cursor as a fresh start", () => {
    expect(parseCursor(new URLSearchParams("epoch=zz&gen=-1&since=x"))).toEqual({ epoch: null, gen: -1, since: 0, live: new Map() });
    expect(parseCursor(new URLSearchParams(`epoch=${E1}&gen=2&since=9`))).toEqual({ epoch: E1, gen: 2, since: 9, live: new Map() });
  });

  it("trusts a passed check for 10 s, and never caches a refusal", async () => {
    let t = 0;
    let open = true;
    const check = vi.fn(async () => {
      if (!open) throw new Error("no");
    });
    const cached = cachedCheck(check, () => t);
    await cached("demo");
    await cached("demo");
    expect(check).toHaveBeenCalledTimes(1);
    t = 10_001;
    open = false;
    await expect(cached("demo")).rejects.toThrow();
    await expect(cached("demo")).rejects.toThrow();
    expect(check).toHaveBeenCalledTimes(3);
  });
});

describe("one hub per process (review R224-1)", () => {
  it("gives concurrent cold callers the same hub, and later callers the cached one", async () => {
    const { sharedHub } = await import("@/lib/events");
    const all = await Promise.all([sharedHub(), sharedHub(), sharedHub()]);
    expect(new Set(all).size).toBe(1);
    expect(await sharedHub()).toBe(all[0]);
  });

  it("starts one daemon wait for simultaneous first polls from several tabs", async () => {
    const { processWide } = await import("@/lib/events");
    const key = Symbol("test hub");
    const d = daemon();
    const make = vi.fn(async () => {
      await flush();
      return d.hub;
    });
    const polls = [0, 1, 2].map(async () => (await processWide(key, make)).poll(["demo"], fresh, 1000));
    await flush();
    await flush();
    expect(make).toHaveBeenCalledTimes(1);
    expect(d.wait).toHaveBeenCalledTimes(1);
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [] });
    for (const r of await Promise.all(polls)) expect(r).toMatchObject({ reset: true, epoch: E1 });
  });

  it("forgets a failed start, so the next caller tries again", async () => {
    const { processWide } = await import("@/lib/events");
    const key = Symbol("failing");
    await expect(processWide(key, async () => Promise.reject(new Error("no")))).rejects.toThrow();
    await expect(processWide(key, async () => 7)).resolves.toBe(7);
  });
});

describe("the change feed through the one wait (plan 127)", () => {
  const A = "01AAAAAAAAAAAAAAAAAAAAAAAA";
  const B = "01BBBBBBBBBBBBBBBBBBBBBBBB";
  const ids = (added: string[] = [], removed: string[] = []) => ({ added, edited: [], removed });
  const changed = (graph: string, from: number, to: number, nodes = ids()) => ({ graph, overflow: false as const, from_rev: from, to_rev: to, nodes, edges: ids() });
  const at = (rev: number, epoch: string | null = E1) => ({ live: new Map([["demo", { epoch, rev }]]) });

  async function following() {
    const d = daemon();
    const live = d.hub.follow("demo", 1000);
    await flush();
    expect(d.last().args.revs).toEqual({ demo: 0 });
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [], changes: [{ graph: "demo", overflow: true, rev: 3 }] });
    expect(await live).toEqual({ epoch: E1, rev: 3 });
    await flush();
    return d;
  }

  it("sends the graph's revision with each wait, and gives a snapshot the revision it is read after", async () => {
    const d = await following();
    expect(d.last().args).toMatchObject({ epoch: E1, revs: { demo: 3 } });
    d.last().answer({ epoch: E1, seq: 0, reset: false, events: [], changes: [changed("demo", 3, 5, ids([A]))] });
    await flush();
    expect(d.last().args.revs).toEqual({ demo: 5 });
  });

  it("answers each tab from its own revision: nothing when current, the changes since merged, or an overflow", async () => {
    const d = await following();
    const r0 = await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(3) }, 30);
    expect(r0.changes.demo ?? null).toBeNull();
    const waiting = d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(3) }, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 0, reset: false, events: [], changes: [changed("demo", 3, 4, ids([A]))] });
    expect((await waiting).changes.demo ?? null).toEqual({ kind: "changed", from: 3, to: 4, nodes: ids([A]), edges: ids() });
    await flush();
    d.last().answer({ epoch: E1, seq: 0, reset: false, events: [], changes: [changed("demo", 4, 5, ids([B], [A]))] });
    await flush();
    // A tab at 3 gets both, merged: A added then removed is gone.
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(3) }, 30)).changes.demo ?? null).toEqual({ kind: "changed", from: 3, to: 5, nodes: ids([B]), edges: ids() });
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(5) }, 30)).changes.demo ?? null).toBeNull();
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(1) }, 30)).changes.demo ?? null).toEqual({ kind: "overflow", rev: 5 });
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0, ...at(0, null) }, 30)).changes.demo ?? null).toEqual({ kind: "overflow", rev: 5 });
    // A tab that does not follow changes gets none.
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 1, since: 0 }, 30)).changes.demo ?? null).toBeNull();
  });

  it("keeps the revisions it follows when another graph joins, and starts again on a new daemon epoch", async () => {
    const d = await following();
    const other = d.hub.poll(["other"], fresh, 1000);
    await flush();
    expect(d.last().args).toMatchObject({ epoch: E1, since: 0, revs: { demo: 3, other: 0 } });
    d.last().answer({ epoch: E1, seq: 0, reset: true, events: [], changes: [{ graph: "other", overflow: true, rev: 1 }] });
    await other;
    expect((await d.hub.poll(["demo"], { epoch: E1, gen: 2, since: 0, ...at(3) }, 30)).changes.demo ?? null).toBeNull();
    await flush();
    d.last().answer({ epoch: E2, seq: 0, reset: true, events: [], changes: [{ graph: "demo", overflow: true, rev: 1 }] });
    await flush();
    expect((await d.hub.poll(["demo"], { epoch: E2, gen: 3, since: 0, ...at(3) }, 30)).changes.demo ?? null).toEqual({ kind: "overflow", rev: 1 });
  });

  it("gives a snapshot no revision when the daemon does not answer in time, and never blocks it", async () => {
    const d = daemon();
    expect(await d.hub.follow("demo", 20)).toBeNull();
    d.last().fail(new UmxError("ui_waiters_busy", "busy"));
    await flush();
    expect(await d.hub.follow("demo", 1000)).toBeNull();
  });

  it("answers one poll for several open graphs with each one's events and its own change (plan 133)", async () => {
    const d = daemon();
    const live = Promise.all([d.hub.follow("demo", 1000), d.hub.follow("other", 1000)]);
    await flush();
    expect(d.last().args.graphs).toEqual(["demo", "other"]);
    d.last().answer({
      epoch: E1,
      seq: 2,
      reset: true,
      events: [ev(1), ev(2, "other")],
      changes: [
        { graph: "demo", overflow: true, rev: 3 },
        { graph: "other", overflow: true, rev: 7 },
      ],
    });
    expect(await live).toEqual([{ epoch: E1, rev: 3 }, { epoch: E1, rev: 7 }]);
    await flush();
    const both = new Map([["demo", { epoch: E1, rev: 3 }], ["other", { epoch: E1, rev: 7 }]]);
    const r = await d.hub.poll(["demo", "other"], { epoch: null, gen: -1, since: 0, live: both }, 30);
    expect(r.events.map((e) => e.graph)).toEqual(["demo", "other"]);
    expect(r.changes).toEqual({});
    const waiting = d.hub.poll(["demo", "other"], { epoch: E1, gen: r.gen, since: r.seq, live: both }, 1000);
    await flush();
    d.last().answer({ epoch: E1, seq: 3, reset: false, events: [ev(3, "other")], changes: [changed("other", 7, 8, ids([B]))] });
    const got = await waiting;
    expect(got.events.map((e) => e.seq)).toEqual([3]);
    expect(got.changes).toEqual({ other: { kind: "changed", from: 7, to: 8, nodes: ids([B]), edges: ids() } });
    // A tab with only demo open sees neither other's event nor its change.
    const one = await d.hub.poll(["demo"], { epoch: E1, gen: r.gen, since: r.seq, live: new Map([["demo", { epoch: E1, rev: 3 }]]) }, 30);
    expect(one).toMatchObject({ events: [], changes: {} });
  });

  it("reads each graph's revision from the query string, and a malformed one as none known", () => {
    const two = parseCursor(new URLSearchParams(`live=demo:${E1}:7&live=other::0`)).live;
    expect([...(two ?? [])]).toEqual([["demo", { epoch: E1, rev: 7 }], ["other", { epoch: null, rev: 0 }]]);
    expect(parseCursor(new URLSearchParams(`live=demo:${E1}:7`)).live?.get("demo")).toEqual({ epoch: E1, rev: 7 });
    expect(parseCursor(new URLSearchParams("live=demo:zz:7")).live?.get("demo")).toEqual({ epoch: null, rev: 0 });
    expect(parseCursor(new URLSearchParams(`live=demo:${E1}:-2`)).live?.get("demo")).toEqual({ epoch: null, rev: 0 });
  });
});

describe("an agent's open reaches every tab (ws009 M1b1)", () => {
  it("gives an open under another graph to a tab that does not show it, and keeps other commands per graph", async () => {
    const d = daemon();
    const p = d.hub.poll(["alpha"], fresh, 1000);
    await flush();
    const open: AgentEvent = { seq: 2, at_ms: 1002, graph: "beta", by: null, command: { kind: "open", graphs: ["beta"], only: false, left_out: 0 } };
    d.last().answer({ epoch: E1, seq: 2, reset: true, events: [ev(1, "beta"), open] });
    expect((await p).events.map((e) => e.seq)).toEqual([2]);
  });
});

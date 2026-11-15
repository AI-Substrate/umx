// The browser feed's ownership (components/use-agent.ts, review R224-2): a feed belongs to one graph, and a failed
// poll never hands one graph's agent state or batch to another.
import { describe, expect, it } from "vitest";
import { EMPTY_FEED, feedFailed, feedFor, feedReplied, follow, type LiveHooks, type OwnedFeed } from "@/components/use-agent";
import type { EventsReply } from "@/lib/events";
import { NO_AGENT, receive } from "@/lib/agent";
import { qid } from "@/lib/graph/keys";
import type { AgentEvent } from "@/lib/umx/socket";

const N = "01NODEAAAAAAAAAAAAAAAAAAAA";
const E1 = "00000000000000e1";
const events: AgentEvent[] = [
  { seq: 1, at_ms: 1, graph: "alpha", by: null, command: { kind: "highlight", nodes: [N], add: false } },
  { seq: 2, at_ms: 1, graph: "alpha", by: null, command: { kind: "filter", show: [], hide: ["type:risk"] } },
  { seq: 3, at_ms: 1, graph: "alpha", by: null, command: { kind: "focus", node: N, hops: 1 } },
];
const down = { code: "daemon_not_running", message: "down" };

function alphaFed(): OwnedFeed {
  const got = receive(NO_AGENT, null, { epoch: E1, seq: 3, reset: true, events });
  return feedReplied({ alias: null, ...EMPTY_FEED }, "alpha", got.state, { n: 1, events: got.act });
}

describe("agent feed ownership", () => {
  it("never relabels alpha's state or batch as beta's when beta's first poll fails", () => {
    const a = alphaFed();
    expect(a.state.highlight.size).toBe(1);
    const b = feedFailed(a, "beta", down);
    const seen = feedFor(b, "beta");
    expect(seen.state).toBe(NO_AGENT);
    expect(seen.batch).toBeNull();
    expect(seen.state.history).toEqual([]);
    expect(seen.state.filter).toBeNull();
    expect(seen.state.focus).toBeNull();
    expect(seen.error).toEqual(down);
    // alpha's feed is not visible as beta's even before beta's poll settles.
    expect(feedFor(a, "beta")).toEqual(EMPTY_FEED);
  });

  it("lets beta recover with its own reply, and never inherits alpha's batch", () => {
    const b = feedFailed(alphaFed(), "beta", down);
    const own = receive(NO_AGENT, null, { epoch: E1, seq: 3, reset: true, events: [] });
    const recovered = feedReplied(b, "beta", own.state, null);
    expect(feedFor(recovered, "beta")).toMatchObject({ state: own.state, batch: null, error: null });
    expect(feedReplied(alphaFed(), "beta", own.state, null).batch).toBeNull();
  });

  it("control: a failed poll on the same graph keeps that graph's own state and batch", () => {
    const a = alphaFed();
    const failed = feedFor(feedFailed(a, "alpha", down), "alpha");
    expect(failed.state).toBe(a.state);
    expect(failed.batch).toBe(a.batch);
    expect(failed.error).toEqual(down);
  });
});

describe("the tab's poll applies each graph change once, in revision order (plan 127)", () => {
  const ids = { added: [] as string[], edited: [] as string[], removed: [] as string[] };
  const changed = (from: number, to: number) => ({ kind: "changed" as const, from, to, nodes: ids, edges: ids });
  const reply = (change: EventsReply["changes"][string] | null, events: AgentEvent[] = []): EventsReply => ({ epoch: E1, gen: 1, seq: 0, reset: false, events, changes: change ? { alpha: change } : {} });

  async function run(replies: EventsReply[], opts: { failFirst?: boolean; removed?: string[] } = {}) {
    const ctrl = new AbortController();
    const paths: string[] = [];
    const applied: { from: number | null; how: string }[] = [];
    let fail = opts.failFirst ?? false;
    let feed: OwnedFeed = { alias: "alpha", ...EMPTY_FEED };
    const hooks: LiveHooks = {
      start: () => ({ epoch: E1, rev: 2 }),
      apply: async (_alias, change, epoch, how) => {
        if (fail) {
          fail = false;
          throw new Error("rows failed");
        }
        applied.push({ from: change.kind === "changed" ? change.from : null, how });
        return { cursor: { epoch, rev: change.kind === "changed" ? change.to : change.rev }, removed: opts.removed ?? [] };
      },
    };
    await follow(["alpha"], () => hooks, ctrl.signal, {
      get: async (path) => {
        paths.push(path);
        const next = replies.shift();
        if (!next) {
          ctrl.abort();
          throw new Error("done");
        }
        return next;
      },
      sleep: async () => {},
      publish: (update) => {
        feed = update(feed);
      },
    });
    return { paths, applied, feed };
  }

  it("skips a duplicate and an out-of-order reply, and sends the revision it is at with each poll", async () => {
    const r = await run([reply(changed(2, 4)), reply(changed(2, 4)), reply(changed(3, 4)), reply(changed(4, 5)), reply({ kind: "overflow", rev: 5 })]);
    expect(r.applied).toEqual([{ from: 2, how: "apply" }, { from: 4, how: "apply" }]);
    expect(r.paths.map((p) => new URLSearchParams(p.split("?")[1]).get("live")?.split(":")[2])).toEqual(["2", "4", "4", "4", "5", "5"]);
  });

  it("offers the same change again after a failed apply, and reloads on an overflow ahead of it", async () => {
    const r = await run([reply(changed(2, 4)), reply(changed(2, 4)), reply({ kind: "overflow", rev: 9 })], { failFirst: true });
    expect(r.applied).toEqual([{ from: 2, how: "apply" }, { from: null, how: "reload" }]);
  });

  it("takes removed nodes out of the agent's highlight", async () => {
    const r = await run([reply(changed(2, 3), [events[0]])], { removed: [qid("alpha", N)] });
    expect(r.feed.state.highlight.size).toBe(0);
  });
});

describe("one poll for several open graphs (plan 133)", () => {
  const ids = { added: [] as string[], edited: [] as string[], removed: [] as string[] };
  const changed = (from: number, to: number) => ({ kind: "changed" as const, from, to, nodes: ids, edges: ids });

  it("names every graph and its revision, applies each graph's change to that graph, and one failure never blocks the other", async () => {
    const ctrl = new AbortController();
    const paths: string[] = [];
    const applied: string[] = [];
    let betaFails = true;
    const starts: Record<string, number> = { alpha: 2, beta: 7 };
    const hooks: LiveHooks = {
      start: (a) => ({ epoch: E1, rev: starts[a] }),
      apply: async (alias, change, epoch) => {
        if (alias === "beta" && betaFails) {
          betaFails = false;
          throw new Error("rows failed");
        }
        applied.push(`${alias}:${change.kind === "changed" ? change.from : "x"}`);
        return { cursor: { epoch, rev: change.kind === "changed" ? change.to : change.rev }, removed: [] };
      },
    };
    const both = (a: number, b: number): EventsReply => ({ epoch: E1, gen: 1, seq: 0, reset: false, events: [], changes: { alpha: changed(a, a + 1), beta: changed(b, b + 1) } });
    const replies = [both(2, 7), both(3, 7)];
    let feed: OwnedFeed = { alias: null, ...EMPTY_FEED };
    await follow(["alpha", "beta"], () => hooks, ctrl.signal, {
      get: async (path) => {
        paths.push(path);
        const next = replies.shift();
        if (!next) {
          ctrl.abort();
          throw new Error("done");
        }
        return next;
      },
      sleep: async () => {},
      publish: (u) => {
        feed = u(feed);
      },
    });
    const q = new URLSearchParams(paths[0].split("?")[1]);
    expect(q.getAll("graph")).toEqual(["alpha", "beta"]);
    expect(q.getAll("live")).toEqual([`alpha:${E1}:2`, `beta:${E1}:7`]);
    // alpha applied at once; beta's failed apply is offered again on the next poll and applied then.
    expect(applied).toEqual(["alpha:2", "alpha:3", "beta:7"]);
    expect(new URLSearchParams(paths[2].split("?")[1]).getAll("live")).toEqual([`alpha:${E1}:4`, `beta:${E1}:8`]);
    expect(feed.alias).toBe("alpha,beta");
  });
});

describe("a cancelled run publishes, applies and prunes nothing (review R261-1)", () => {
  const ids = { added: [] as string[], edited: [] as string[], removed: [] as string[] };
  const change = { kind: "changed" as const, from: 2, to: 3, nodes: ids, edges: ids };
  const alphaReply: EventsReply = { epoch: E1, gen: 1, seq: 2, reset: true, events: events.slice(0, 2), changes: { alpha: change } };
  const betaFeed: OwnedFeed = feedReplied({ alias: null, ...EMPTY_FEED }, "beta", receive(NO_AGENT, null, { epoch: E1, seq: 1, reset: true, events: [{ ...events[1], graph: "beta" }] }).state, null);

  function deferred<T>() {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
  }

  /** The tab's feed as React holds it: updaters are queued, and run only when flushed. */
  function store(start: OwnedFeed) {
    let feed = start;
    const queued: ((f: OwnedFeed) => OwnedFeed)[] = [];
    return {
      publish: (u: (f: OwnedFeed) => OwnedFeed) => queued.push(u),
      flush: () => {
        while (queued.length) feed = queued.shift()!(feed);
      },
      set: (f: OwnedFeed) => {
        feed = f;
      },
      get: () => feed,
      queued: () => queued.length,
    };
  }

  function alpha(get: () => Promise<EventsReply>, apply: LiveHooks["apply"]) {
    const ctrl = new AbortController();
    const s = store({ alias: "alpha", ...EMPTY_FEED });
    const calls = { get: 0, apply: 0 };
    const hooks: LiveHooks = {
      start: () => ({ epoch: E1, rev: 2 }),
      apply: (...a) => {
        calls.apply++;
        return apply(...a);
      },
    };
    const run = follow(["alpha"], () => hooks, ctrl.signal, {
      get: () => {
        calls.get++;
        return get();
      },
      sleep: async () => {},
      publish: s.publish,
    });
    return { ctrl, s, calls, run };
  }

  it("an alpha reply that settles after the switch to beta is dropped: nothing published or applied, beta intact", async () => {
    const reply = deferred<EventsReply>();
    const a = alpha(() => reply.promise, async () => ({ cursor: { epoch: E1, rev: 3 }, removed: [] }));
    await Promise.resolve();
    a.ctrl.abort();
    a.s.set(betaFeed);
    reply.resolve(alphaReply);
    await a.run;
    a.s.flush();
    expect(a.calls).toEqual({ get: 1, apply: 0 });
    expect(a.s.get()).toBe(betaFeed);
    expect(feedFor(a.s.get(), "beta").state.filter).not.toBeNull();
  });

  it("an alpha apply that settles after cancellation, with removed ids, neither publishes nor prunes beta, and polls no more", async () => {
    const applying = deferred<{ cursor: { epoch: string; rev: number }; removed: string[] }>();
    let gets = 0;
    const a = alpha(async () => (gets++ === 0 ? alphaReply : new Promise<EventsReply>(() => {})), () => applying.promise);
    for (let i = 0; i < 5 && a.calls.apply === 0; i++) await Promise.resolve();
    expect(a.calls.apply).toBe(1);
    a.s.flush();
    a.ctrl.abort();
    const beta = feedReplied(betaFeed, "beta", { ...betaFeed.state, highlight: new Set([N]) }, null);
    a.s.set(beta);
    applying.resolve({ cursor: { epoch: E1, rev: 3 }, removed: [N] });
    await a.run;
    a.s.flush();
    expect(a.calls.get).toBe(1);
    expect(a.s.get()).toBe(beta);
    expect(a.s.get().state.highlight.has(N)).toBe(true);
  });

  it("an update queued before cancellation and run after it keeps the newer feed", async () => {
    let gets = 0;
    const a = alpha(async () => (gets++ === 0 ? alphaReply : new Promise<EventsReply>(() => {})), async () => ({ cursor: { epoch: E1, rev: 3 }, removed: [] }));
    for (let i = 0; i < 5 && a.s.queued() === 0; i++) await Promise.resolve();
    expect(a.s.queued()).toBeGreaterThan(0);
    a.ctrl.abort();
    a.s.set(betaFeed);
    a.s.flush();
    await a.run;
    expect(a.s.get()).toBe(betaFeed);
  });

  it("an active run still publishes its own replies (the control)", async () => {
    let gets = 0;
    const a = alpha(async () => (gets++ === 0 ? alphaReply : new Promise<EventsReply>(() => {})), async () => ({ cursor: { epoch: E1, rev: 3 }, removed: [] }));
    for (let i = 0; i < 10 && a.calls.get < 2; i++) await Promise.resolve();
    a.s.flush();
    expect(a.s.get().alias).toBe("alpha");
    expect(a.s.get().state.highlight.has(qid("alpha", N))).toBe(true);
    a.ctrl.abort();
  });
});

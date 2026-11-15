// The person's selection (lib/selection, plan 117): click, shift-click, lasso and Esc, and how a tab publishes it:
// debounced, paused, and cleared for the old graph on a switch.
import { describe, expect, it } from "vitest";
import { shareTarget } from "@/components/use-selection";
import { qid } from "@/lib/graph/keys";
import {
  addMany,
  closePanel,
  dropRemoved,
  DEBOUNCE_MS,
  escape,
  inPolygon,
  MAX_SELECTION,
  nodesInLasso,
  noSelection,
  perGraph,
  pickOne,
  Publisher,
  RETRY_MIN_MS,
  type ShareState,
  toggleOne,
} from "@/lib/selection";

const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const C = "01NODECCCCCCCCCCCCCCCCCCCC";
const D = "01NODEDDDDDDDDDDDDDDDDDDDD";

describe("selecting", () => {
  it("click selects that node alone and opens it in the panel", () => {
    const s = pickOne(addMany(noSelection("demo"), [A, B]).next, C);
    expect(s).toEqual({ graph: "demo", ids: [C], primary: C });
  });

  it("shift-click adds a node, and removes it when it is already selected", () => {
    let s = pickOne(noSelection("demo"), A);
    s = toggleOne(s, B);
    expect(s).toEqual({ graph: "demo", ids: [A, B], primary: B });
    s = toggleOne(s, A);
    expect(s).toEqual({ graph: "demo", ids: [B], primary: B });
    // Removing the panel's node closes the panel; the rest stay selected.
    s = toggleOne(toggleOne(s, C), C);
    expect(s).toEqual({ graph: "demo", ids: [B], primary: null });
  });

  it("lasso adds what is not yet selected, keeps the panel's node, and stops at the cap", () => {
    const s = addMany(pickOne(noSelection("demo"), A), [B, A, C, B]);
    expect(s).toEqual({ next: { graph: "demo", ids: [A, B, C], primary: A }, left: 0 });
    const full = addMany(noSelection("demo"), Array.from({ length: MAX_SELECTION - 1 }, (_v, i) => `n${i}`)).next;
    const capped = addMany(full, [A, B, C]);
    expect(capped.next.ids.length).toBe(MAX_SELECTION);
    expect(capped.next.ids.at(-1)).toBe(A);
    expect(capped.left).toBe(2);
    expect(toggleOne(capped.next, D)).toBe(capped.next);
  });

  it("Esc clears focus first, then the whole selection", () => {
    const s = addMany(pickOne(noSelection("demo"), A), [B]).next;
    const first = escape(2, s);
    expect(first).toEqual({ focusDepth: 0, selection: s });
    expect(escape(first.focusDepth, first.selection)).toEqual({ focusDepth: 0, selection: noSelection("demo") });
  });

  it("closing the panel keeps the selection", () => {
    const s = addMany(pickOne(noSelection("demo"), A), [B]).next;
    expect(closePanel(s)).toEqual({ graph: "demo", ids: [A, B], primary: null });
  });
});

describe("the lasso", () => {
  const square = [0, 0, 10, 0, 10, 10, 0, 10];

  it("finds points inside a polygon, concave ones too", () => {
    expect(inPolygon(square, 5, 5)).toBe(true);
    expect(inPolygon(square, 11, 5)).toBe(false);
    const u = [0, 0, 3, 0, 3, 8, 7, 8, 7, 0, 10, 0, 10, 10, 0, 10];
    expect(inPolygon(u, 5, 4)).toBe(false);
    expect(inPolygon(u, 5, 9)).toBe(true);
    expect(inPolygon(u, 1, 4)).toBe(true);
  });

  it("never selects a hidden node, even inside it", () => {
    const nodes = [
      { id: A, x: 5, y: 5, hidden: false },
      { id: B, x: 6, y: 6, hidden: true },
      { id: C, x: 15, y: 5, hidden: false },
    ];
    expect(nodesInLasso(square, nodes)).toEqual([A]);
    expect(nodesInLasso([0, 0, 10, 10], nodes)).toEqual([]);
  });
});

/**
 * A publisher on fake timers against a stand-in for the daemon's per-graph selection (`remote`), which other tabs may
 * have written. Records every request and what the top bar would show; `fail` decides which requests fail.
 */
function rig(seed: [string, string[]][] = []) {
  const remote = new Map<string, string[]>(seed);
  const posts: [string, string[]][] = [];
  const states: ShareState[] = [];
  const timers = new Map<number, { fn: () => void; ms: number }>();
  let id = 0;
  let pending: (() => void)[] = [];
  let fail: (graph: string, nodes: readonly string[]) => boolean = () => false;
  const pub = new Publisher({
    post: (graph, nodes) =>
      new Promise((resolve, reject) => {
        posts.push([graph, [...nodes]]);
        pending.push(() => {
          if (fail(graph, nodes)) return reject(new Error("down"));
          remote.set(graph, [...nodes]);
          resolve();
        });
      }),
    changed: (s) => states.push(s),
    setTimer: (fn, ms) => {
      timers.set(++id, { fn, ms });
      return id;
    },
    clearTimer: (t) => timers.delete(t as number),
    asIssue: (e) => ({ code: "daemon_not_running", message: String(e) }),
  });
  /** Runs the timers due now: the debounces, and retries only when asked. */
  const tick = (retries = false) => {
    for (const [k, t] of [...timers]) {
      if (!retries && t.ms !== DEBOUNCE_MS) continue;
      timers.delete(k);
      t.fn();
    }
  };
  /** Answers every request sent so far, then any it led to. */
  const settle = async () => {
    for (let i = 0; i < 20; i++) {
      const now = pending;
      pending = [];
      for (const answer of now) answer();
      await new Promise((r) => setTimeout(r, 0));
    }
  };
  const failures = () => (states.at(-1)?.failures ?? []).map((f) => f.graph);
  return { pub, remote, posts, states, tick, settle, timers, failures, failWhen: (f: typeof fail) => (fail = f) };
}

describe("publishing the selection", () => {
  it("clears graphs that left as soon as the selection resets, even when the next set never loads (review B3)", async () => {
    const r = rig();
    r.pub.update(perGraph(["alpha"], [qid("alpha", A)]));
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["alpha", [A]]]);
    // The app resets the selection to no graph while it reads beta; beta's read then fails, so no view arrives. That
    // reset alone tells the publisher no graph is open.
    expect(shareTarget(noSelection(null), [])).toEqual(new Map());
    r.pub.update(shareTarget(noSelection(null), [])!);
    await r.settle();
    expect(r.posts.slice(1)).toEqual([["alpha", []]]);
    expect(r.remote.get("alpha")).toEqual([]);
    // A failed publish waiting to retry is replaced by the clear, never re-sent with the old ids.
    const f = rig();
    f.failWhen((g, nodes) => nodes.length > 0);
    f.pub.update(perGraph(["alpha"], [qid("alpha", A)]));
    f.tick();
    await f.settle();
    f.pub.update(shareTarget(noSelection(null), [])!);
    await f.settle();
    f.tick(true);
    await f.settle();
    expect(f.posts.slice(1).every(([, nodes]) => nodes.length === 0)).toBe(true);
    // A selection made in other graphs than those open publishes nothing until it is reset for them.
    expect(shareTarget({ graph: "alpha", ids: [], primary: null }, ["beta"])).toBeNull();
  });

  it("publishes to each open graph its own ids, and clears a graph that leaves the canvas first (plan 133)", async () => {
    const r = rig([["gamma", [C]]]);
    const keys = [qid("alpha", A), qid("beta", A), qid("beta", B)];
    expect([...perGraph(["alpha", "beta", "gamma"], keys)]).toEqual([["alpha", [A]], ["beta", [A, B]], ["gamma", []]]);
    r.pub.update(perGraph(["alpha", "beta", "gamma"], keys));
    r.tick();
    await r.settle();
    // gamma held another tab's selection: an ordinary change never clears it.
    expect(r.posts).toEqual([["alpha", [A]], ["beta", [A, B]]]);
    expect(r.remote.get("gamma")).toEqual([C]);
    // beta leaves: it is cleared at once (leaving always sends, review R235-1); alpha's unchanged ids are not sent
    // again, and gamma, still open with nothing selected here, is left as the other tab set it.
    r.pub.update(perGraph(["alpha", "gamma"], [qid("alpha", A)]));
    await r.settle();
    r.tick();
    await r.settle();
    expect(r.posts.slice(2)).toEqual([["beta", []]]);
    expect(r.remote.get("gamma")).toEqual([C]);
    // As the app does on a set change, the selection resets: alpha, published to before, is cleared too; gamma, never
    // sent to from here, still is not (plan 133 review).
    r.pub.update(perGraph(["alpha", "gamma", "beta"], []));
    r.tick();
    await r.settle();
    expect(r.posts.slice(3)).toEqual([["alpha", []]]);
    expect(r.remote.get("gamma")).toEqual([C]);
    r.pub.update(perGraph(["alpha", "gamma", "beta"], [qid("alpha", A)]));
    r.tick();
    await r.settle();
    // Pausing clears every open graph at once.
    r.pub.setSharing(false);
    await r.settle();
    expect(r.posts.slice(5)).toEqual([["alpha", []], ["gamma", []], ["beta", []]]);
  });

  it("publishes a burst of changes once, after the debounce", async () => {
    const r = rig();
    r.pub.update("demo", []);
    r.pub.update("demo", [A]);
    r.pub.update("demo", [A, B]);
    r.pub.update("demo", [A, B, C]);
    expect(r.posts).toEqual([]);
    expect(DEBOUNCE_MS).toBeGreaterThanOrEqual(200);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["demo", [A, B, C]]]);
    // The same selection again publishes nothing; an emptied one publishes empty.
    r.pub.update("demo", [A, B, C]);
    r.tick();
    r.pub.update("demo", []);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["demo", [A, B, C]], ["demo", []]]);
  });

  it("opening a graph with nothing selected never clears what another tab shared there", async () => {
    const r = rig([["demo", [A]]]);
    r.pub.update("demo", []);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([]);
    expect(r.remote.get("demo")).toEqual([A]);
  });

  it("pause clears the daemon's copy at once and blocks publishing; unpause republishes", async () => {
    const r = rig();
    r.pub.update("demo", [A]);
    r.tick();
    await r.settle();
    r.pub.update("demo", [A, B]);
    r.pub.setSharing(false);
    expect([...r.timers.values()]).toEqual([]);
    await r.settle();
    expect(r.posts).toEqual([["demo", [A]], ["demo", []]]);
    expect(r.states.at(-1)).toEqual({ sharing: false, failures: [] });
    r.pub.update("demo", [A, B, C]);
    r.tick();
    await r.settle();
    expect(r.posts).toHaveLength(2);
    r.pub.setSharing(true);
    await r.settle();
    expect(r.posts.at(-1)).toEqual(["demo", [A, B, C]]);
    expect(r.remote.get("demo")).toEqual([A, B, C]);
  });

  it("pause in a fresh tab clears a selection another tab left in the daemon (R235-1)", async () => {
    const r = rig([["demo", [A]]]);
    r.pub.update("demo", []);
    r.pub.setSharing(false);
    await r.settle();
    expect(r.posts).toEqual([["demo", []]]);
    expect(r.remote.get("demo")).toEqual([]);
  });

  it("a switch before the first debounce clears the old graph before the new one's selection (R235-1)", async () => {
    const r = rig([["alpha", [A]]]);
    r.pub.update("alpha", [A]);
    r.pub.update("beta", [B]);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["alpha", []], ["beta", [B]]]);
    expect(r.remote.get("alpha")).toEqual([]);
  });

  it("a graph switch clears the old graph even when this tab last sent it empty", async () => {
    const r = rig();
    r.pub.update("alpha", [A, B]);
    r.tick();
    await r.settle();
    r.pub.update("alpha", []);
    r.tick();
    await r.settle();
    r.remote.set("alpha", [C]); // another tab
    r.pub.update("beta", []);
    r.pub.update("beta", [C]);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["alpha", [A, B]], ["alpha", []], ["alpha", []], ["beta", [C]]]);
    expect(r.remote.get("alpha")).toEqual([]);
  });

  it("pause and an empty unpause are sent even when this tab last sent empty and another writer changed it (R235-1)", async () => {
    const r = rig();
    r.pub.update("demo", [A]);
    r.tick();
    r.pub.update("demo", []);
    r.tick();
    await r.settle();
    expect(r.posts).toEqual([["demo", [A]], ["demo", []]]);
    r.remote.set("demo", [B]); // another tab
    r.pub.setSharing(false);
    await r.settle();
    expect(r.remote.get("demo")).toEqual([]);
    r.remote.set("demo", [C]); // and again, while this tab is paused with nothing selected
    r.pub.setSharing(true);
    await r.settle();
    expect(r.posts.slice(2)).toEqual([["demo", []], ["demo", []]]);
    expect(r.remote.get("demo")).toEqual([]);
  });

  it("sends one publish at a time, in order, keeping only the newest queued for a graph", async () => {
    const r = rig();
    r.pub.update("demo", [A]);
    r.tick();
    r.pub.update("demo", [A, B]);
    r.tick();
    r.pub.update("demo", [A, B, C]);
    r.tick();
    expect(r.posts).toEqual([["demo", [A]]]);
    await r.settle();
    expect(r.posts).toEqual([["demo", [A]], ["demo", [A, B, C]]]);
  });

  it("shows a failed pause clear while paused and empty, and retries it until it succeeds (R235-2)", async () => {
    const r = rig();
    r.pub.update("demo", [A]);
    r.tick();
    await r.settle();
    r.failWhen(() => true);
    r.pub.update("demo", []);
    r.pub.setSharing(false);
    await r.settle();
    expect(r.remote.get("demo")).toEqual([A]);
    expect(r.states.at(-1)).toMatchObject({ sharing: false, failures: [{ graph: "demo", error: { code: "daemon_not_running" } }] });
    // The retry waits RETRY_MIN_MS, then the next one twice that; still failing, the warning stays.
    expect([...r.timers.values()].map((t) => t.ms)).toEqual([RETRY_MIN_MS]);
    r.tick(true);
    await r.settle();
    expect([...r.timers.values()].map((t) => t.ms)).toEqual([RETRY_MIN_MS * 2]);
    expect(r.failures()).toEqual(["demo"]);
    r.failWhen(() => false);
    r.tick(true);
    await r.settle();
    expect(r.remote.get("demo")).toEqual([]);
    expect(r.failures()).toEqual([]);
    expect(r.posts.slice(1)).toEqual([["demo", []], ["demo", []], ["demo", []]]);
  });

  it("another graph's success never hides alpha's failed switch clear; alpha's own retry does (R235-2)", async () => {
    const r = rig();
    r.pub.update("alpha", [A]);
    r.tick();
    await r.settle();
    r.failWhen((graph) => graph === "alpha");
    r.pub.update("beta", [B]);
    r.tick();
    await r.settle();
    expect(r.remote.get("alpha")).toEqual([A]);
    expect(r.remote.get("beta")).toEqual([B]);
    expect(r.failures()).toEqual(["alpha"]);
    r.pub.update("beta", [B, C]);
    r.tick();
    await r.settle();
    expect(r.failures()).toEqual(["alpha"]);
    r.failWhen(() => false);
    r.tick(true);
    await r.settle();
    expect(r.posts.at(-1)).toEqual(["alpha", []]);
    expect(r.remote.get("alpha")).toEqual([]);
    expect(r.failures()).toEqual([]);
  });

  it("a retry sends what the graph should hold now, and a newer publish replaces it", async () => {
    const r = rig();
    r.failWhen(() => true);
    r.pub.update("demo", [A]);
    r.tick();
    await r.settle();
    expect(r.failures()).toEqual(["demo"]);
    r.failWhen(() => false);
    r.pub.update("demo", [A, B]);
    r.tick();
    await r.settle();
    // The change went out at once and dropped the retry; the failure is resolved.
    expect([...r.timers.values()]).toEqual([]);
    expect(r.posts).toEqual([["demo", [A]], ["demo", [A, B]]]);
    expect(r.failures()).toEqual([]);
    r.failWhen(() => true);
    r.pub.update("demo", [C]);
    r.tick();
    await r.settle();
    r.pub.stop();
    expect([...r.timers.values()]).toEqual([]);
  });
});

describe("live removals (plan 127)", () => {
  const [A, B, C] = ["A", "B", "C"].map((c) => `01${c.repeat(24)}`);
  it("drops removed nodes from the selection and says when the panel's node went", () => {
    const s = { graph: "demo", ids: [A, B, C], primary: B };
    expect(dropRemoved(s, new Set([C]))).toEqual({ next: { graph: "demo", ids: [A, B], primary: B }, primaryGone: false });
    expect(dropRemoved(s, new Set([B]))).toEqual({ next: { graph: "demo", ids: [A, C], primary: null }, primaryGone: true });
    expect(dropRemoved(s, new Set(["01ZZZZZZZZZZZZZZZZZZZZZZZZ"])).next).toBe(s);
  });
});

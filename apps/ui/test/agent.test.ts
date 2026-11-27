// What agents set on the view (lib/agent.ts, plan 114): state from events, filters on and off the chips, when the
// camera may move, and the strip's words.
import { describe, expect, it } from "vitest";
import {
  activeNotes, agentCentre, agentFilter, agentOff, applyAgentFilter, cameraAsk, centreSelects, decide, dropRemovedNodes, Gesture, describe as words, HISTORY_MAX, LEDGER0, type Ledger, NO_AGENT,
  openIgnored, openLanded, openMayMove, openPlan, openTail, personSets, qualifyEvent, raiseTitle, readEach, receive, reduce, STALE_MS, TOUCH_MS, undoAgentFilter,
} from "@/lib/agent";
import { Raiser } from "@/lib/raise";
import { type Filters, NO_FILTERS, nodeShown, toggleChip } from "@/lib/graph/filters";
import { qid } from "@/lib/graph/keys";
import type { AgentEvent, UiCommand } from "@/lib/umx/socket";

const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const C = "01NODECCCCCCCCCCCCCCCCCCCC";
const E1 = "00000000000000e1";
const E2 = "00000000000000e2";
let next = 0;
const ev = (command: UiCommand, more: Partial<AgentEvent> = {}): AgentEvent => ({ seq: ++next, at_ms: 1_000, graph: "demo", by: null, command, ...more });
/** As the view reduces them: node ids qualified by the event's graph (plan 133). */
const qev = (command: UiCommand, more: Partial<AgentEvent> = {}) => qualifyEvent(ev(command, more));
const [qA, qB, qC] = [A, B, C].map((n) => qid("demo", n));

describe("agent state", () => {
  it("highlight replaces, or adds with add", () => {
    let s = reduce(NO_AGENT, qev({ kind: "highlight", nodes: [A, B], add: false }));
    s = reduce(s, qev({ kind: "highlight", nodes: [C], add: false }));
    expect([...s.highlight]).toEqual([qC]);
    s = reduce(s, qev({ kind: "highlight", nodes: [A], add: true }));
    expect([...s.highlight].sort()).toEqual([qA, qC]);
  });

  it("clear removes only what agents set: highlight, focus and filter", () => {
    let s = reduce(NO_AGENT, qev({ kind: "highlight", nodes: [A], add: false }));
    s = reduce(s, qev({ kind: "focus", node: B, hops: 2 }));
    s = reduce(s, qev({ kind: "filter", show: [], hide: ["type:option"] }));
    const cleared = reduce(s, qev({ kind: "clear" }));
    expect(cleared.highlight.size).toBe(0);
    expect(cleared.focus).toBeNull();
    expect(cleared.filter).toBeNull();
    expect(cleared.history[0].command.kind).toBe("clear");
  });

  it("keeps the newest history first, at most HISTORY_MAX", () => {
    let s = NO_AGENT;
    for (let i = 0; i < HISTORY_MAX + 5; i++) s = reduce(s, qev({ kind: "fit", nodes: [] }));
    expect(s.history).toHaveLength(HISTORY_MAX);
    expect(s.history[0].seq).toBeGreaterThan(s.history[1].seq);
  });

  it("acts within the graph each command names, with equal ids in two graphs kept apart (plan 133)", () => {
    const [aA, bA, bB] = [qid("alpha", A), qid("beta", A), qid("beta", B)];
    let s = reduce(NO_AGENT, qev({ kind: "highlight", nodes: [A], add: false }, { graph: "alpha" }));
    s = reduce(s, qev({ kind: "highlight", nodes: [A, B], add: false }, { graph: "beta" }));
    // beta's highlight replaced only beta's part: alpha's A stays, and beta's A is another node.
    expect([...s.highlight].sort()).toEqual([aA, bA, bB].sort());
    s = reduce(s, qev({ kind: "highlight", nodes: [B], add: false }, { graph: "beta" }));
    expect([...s.highlight].sort()).toEqual([aA, bB].sort());
    s = reduce(s, qev({ kind: "focus", node: A, hops: 1 }, { graph: "alpha" }));
    s = reduce(s, qev({ kind: "filter", show: [], hide: ["type:risk"] }, { graph: "alpha" }));
    // clear in beta takes off beta's highlight only; alpha's highlight, focus and filter stay.
    const cleared = reduce(s, qev({ kind: "clear" }, { graph: "beta" }));
    expect([...cleared.highlight]).toEqual([aA]);
    expect(cleared.focus).toEqual({ node: aA, hops: 1 });
    expect(cleared.filter).toMatchObject({ hide: ["type:risk"], graph: "alpha" });
    // Notes follow the same rule: beta's clear leaves alpha's notes on screen.
    const noted = reduce(reduce(NO_AGENT, qev({ kind: "highlight", nodes: [A], add: false, note: "alpha's" }, { graph: "alpha" })), qev({ kind: "highlight", nodes: [B], add: false, note: "beta's" }, { graph: "beta" }));
    expect([...activeNotes(reduce(noted, qev({ kind: "clear" }, { graph: "beta" })))]).toEqual([[aA, "alpha's"]]);
    const all = reduce(cleared, qev({ kind: "clear" }, { graph: "alpha" }));
    expect(all.highlight.size).toBe(0);
    expect(all.focus).toBeNull();
    expect(all.filter).toBeNull();
  });
});

describe("replies", () => {
  const h = ev({ kind: "highlight", nodes: [A], add: false }, { seq: 1 });
  const f = ev({ kind: "focus", node: B, hops: 1 }, { seq: 2 });
  const g = ev({ kind: "filter", show: [], hide: ["type:option"] }, { seq: 3 });

  it("a first reset rebuilds the state and acts on every kept event", () => {
    const r = receive(NO_AGENT, null, { epoch: E1, seq: 3, reset: true, events: [h, f, g] });
    expect([...r.state.highlight]).toEqual([qA]);
    expect(r.state.focus).toEqual({ node: qB, hops: 1 });
    expect(r.act.map((e) => e.seq)).toEqual([1, 2, 3]);
    expect(r.applied).toEqual({ epoch: E1, seq: 3 });
  });

  it("a later reset in the same epoch rebuilds, but never acts again on what was acted on", () => {
    const first = receive(NO_AGENT, null, { epoch: E1, seq: 3, reset: true, events: [h, f, g] });
    const clear = ev({ kind: "clear" }, { seq: 4 });
    const again = receive(first.state, first.applied, { epoch: E1, seq: 4, reset: true, events: [h, f, g, clear] });
    expect(again.act.map((e) => e.seq)).toEqual([4]);
    expect(again.state.highlight.size).toBe(0);
    // A new daemon epoch is a new ring: everything in it is new.
    const restarted = receive(again.state, again.applied, { epoch: E2, seq: 1, reset: true, events: [{ ...h, seq: 1 }] });
    expect(restarted.act.map((e) => e.seq)).toEqual([1]);
    expect([...restarted.state.highlight]).toEqual([qA]);
  });

  it("a live reply adds to the state and acts on its events", () => {
    const first = receive(NO_AGENT, null, { epoch: E1, seq: 1, reset: true, events: [h] });
    const live = receive(first.state, first.applied, { epoch: E1, seq: 2, reset: false, events: [f] });
    expect([...live.state.highlight]).toEqual([qA]);
    expect(live.act).toEqual([qualifyEvent(f)]);
  });
});

describe("agent filters", () => {
  it("turns named chips off or on, and Undo puts back only what it changed", () => {
    const start: Filters = { relations: new Set(["cites"]), types: new Set(), statuses: new Set() };
    const { filters, changes } = applyAgentFilter(start, { show: ["relation:cites", "status:open"], hide: ["type:option", "bogus", "type:"] });
    expect([...filters.types]).toEqual(["option"]);
    expect([...filters.relations]).toEqual([]);
    // status:open was already on: no change, no pulse.
    expect(changes.map((c) => `${c.dim}:${c.key}`).sort()).toEqual(["relations:cites", "types:option"]);
    expect(undoAgentFilter(filters, changes)).toEqual(start);
  });

  it("Undo leaves a chip the person has set since", () => {
    const { filters, changes } = applyAgentFilter(NO_FILTERS, { show: [], hide: ["type:option", "type:problem"] });
    const touched = { ...filters, types: new Set(["problem"]) };
    expect([...undoAgentFilter(touched, changes).types]).toEqual([]);
    const moved = { ...filters, types: new Set(["problem", "option", "risk"]) };
    expect([...undoAgentFilter(moved, changes).types]).toEqual(["risk"]);
  });

  it("never shows what the provenance and retired toggles hide, whatever it names", () => {
    const visible = { provenance: false, retired: false };
    const { filters } = applyAgentFilter(NO_FILTERS, { show: ["type:session", "status:retired", "type:agent"], hide: [] });
    expect(nodeShown({ kind: "session", status: "open" }, visible, filters)).toBe(false);
    expect(nodeShown({ kind: "problem", status: "retired" }, visible, filters)).toBe(false);
    // It never returns toggles at all: only chips.
    expect(Object.keys(applyAgentFilter(NO_FILTERS, { show: [], hide: [] }).filters).sort()).toEqual(["relations", "statuses", "types"]);
  });
});

describe("who owns a chip (review R224-3)", () => {
  // As graph-app wires them: a chip click, Reset filters, an agent filter, and Undo or clear.
  const click = (l: Ledger, key: string) => personSets(l, toggleChip(l.filters, "types", key), [`types:${key}`]);
  const reset = (l: Ledger) => personSets(l, NO_FILTERS, "all");
  const types = (l: Ledger) => [...l.filters.types].sort();

  it("never re-hides a chip after the person's Reset filters: clear, Undo or a replacing filter", () => {
    let l = click(click(LEDGER0, "risk"), "option");
    expect(types(l)).toEqual(["option", "risk"]);
    l = agentFilter(l, 1, { show: ["type:risk"], hide: [] });
    expect(types(l)).toEqual(["option"]);
    l = reset(l);
    expect(types(l)).toEqual([]);
    expect(types(agentOff(l))).toEqual([]);
    expect(types(agentFilter(l, 2, { show: [], hide: ["relation:cites"] }))).toEqual([]);
  });

  it("leaves a chip the person changed twice after the agent's change as the person left it", () => {
    let l = agentFilter(LEDGER0, 1, { show: [], hide: ["type:option"] });
    l = click(click(l, "option"), "option");
    expect(types(l)).toEqual(["option"]);
    expect(types(agentOff(l))).toEqual(["option"]);
    expect(types(agentFilter(l, 2, { show: [], hide: [] }))).toEqual(["option"]);
    // Once is enough: shown by the person, it stays shown.
    const once = click(agentFilter(LEDGER0, 1, { show: [], hide: ["type:option"] }), "option");
    expect(types(agentOff(once))).toEqual([]);
  });

  it("controls: Undo with no person edit restores the chips; an unrelated chip the person set is kept", () => {
    const start = click(LEDGER0, "risk");
    const applied = agentFilter(start, 1, { show: ["type:risk"], hide: ["type:option"] });
    expect(types(applied)).toEqual(["option"]);
    expect(agentOff(applied).filters).toEqual(start.filters);
    const mixed = click(applied, "problem");
    expect(types(agentOff(mixed))).toEqual(["problem", "risk"]);
    // Show (reveal) takes over the chips it changes, and only those.
    const shown = personSets(applied, { ...applied.filters, types: new Set() });
    expect(shown.agent.map((c) => c.key)).toEqual(["risk"]);
    expect(types(agentOff(shown))).toEqual(["risk"]);
  });
});

describe("the camera", () => {
  const ask = cameraAsk(ev({ kind: "focus", node: A, hops: 1 }, { at_ms: 100_000 }))!;
  const at = { now: 100_000, follow: true, visible: true, lastTouch: 0, busy: false, shown: () => true };

  it("selects with a centring only when the newest centre on that node asked to, and never asks the camera", () => {
    const sel = qev({ kind: "focus", node: A, hops: 1, centre: true, select: true });
    const plain = qev({ kind: "focus", node: A, hops: 1, centre: true });
    expect(centreSelects([sel], qA)).toBe(true);
    expect(centreSelects([sel, plain], qA)).toBe(false);
    expect(centreSelects([sel], qB)).toBe(false);
    expect(centreSelects([qev({ kind: "focus", node: A, hops: 1, select: true })], qA)).toBe(false);
    expect(cameraAsk(sel)).toBeNull();
    expect(words(sel, () => "Gear").action).toBe("selected and centred the view on");
  });

  it("carries a focus --select to the camera move, qualified, and says it selects", () => {
    const e = qualifyEvent(ev({ kind: "focus", node: A, hops: 1, select: true }));
    expect(cameraAsk(e)).toMatchObject({ kind: "focus", nodes: [qA], select: true });
    expect(cameraAsk(ev({ kind: "focus", node: A, hops: 1 }))).not.toHaveProperty("select");
    expect(words(e, () => "Gear").action).toBe("focused and selected");
  });

  it("moves when following, visible and untouched", () => {
    expect(decide(ask, at)).toBe("move");
    expect(cameraAsk(ev({ kind: "highlight", nodes: [A], add: false }))).toBeNull();
    expect(cameraAsk(ev({ kind: "fit", nodes: [] }))).toMatchObject({ kind: "fit", nodes: [] });
  });

  it("asks instead within 5 s of a touch, or with follow off (Agent wants to show X)", () => {
    expect(decide(ask, { ...at, lastTouch: at.now - TOUCH_MS + 1 })).toBe("prompt");
    expect(decide(ask, { ...at, lastTouch: at.now - TOUCH_MS - 1 })).toBe("move");
    expect(decide(ask, { ...at, follow: false })).toBe("prompt");
  });

  it("waits for a hidden tab, drops a stale ask, and never moves to a node that is not shown", () => {
    expect(decide(ask, { ...at, visible: false })).toBe("defer");
    expect(decide(ask, { ...at, now: ask.at_ms + STALE_MS + 1 })).toBe("drop");
    expect(decide(ask, { ...at, now: ask.at_ms + STALE_MS })).toBe("move");
    expect(decide(ask, { ...at, shown: () => false })).toBe("not_shown");
    const fitAll = cameraAsk(ev({ kind: "fit", nodes: [] }, { at_ms: 100_000 }))!;
    expect(decide(fitAll, { ...at, shown: () => false })).toBe("move");
  });
});

describe("a gesture held past TOUCH_MS (review R224-4)", () => {
  const T0 = 1_000_000;
  const ask = (now: number) => cameraAsk(ev({ kind: "focus", node: A, hops: 1 }, { at_ms: now }))!;
  const fit = (now: number) => cameraAsk(ev({ kind: "fit", nodes: [A] }, { at_ms: now }))!;
  const at = (g: Gesture, now: number) => ({ now, follow: true, visible: true, lastTouch: g.lastTouch, busy: g.active, shown: () => true });

  it("never moves the camera mid-drag, with the pointer moving or held still", () => {
    const g = new Gesture();
    g.press(1, T0);
    for (let t = T0 + 1_000; t <= T0 + 8_000; t += 1_000) g.move(t);
    const now = T0 + 8_000 + TOUCH_MS + 1;
    // Still pressed, last moved more than TOUCH_MS ago: the gesture is what counts, not its last move.
    expect(decide(ask(now), at(g, now))).toBe("prompt");
    expect(decide(fit(now), at(g, now))).toBe("prompt");
  });

  it("counts from the release, or the cancel, and moves once the person has been idle TOUCH_MS", () => {
    for (const end of ["release", "cancel"] as const) {
      const g = new Gesture();
      g.press(7, T0);
      g.move(T0 + 6_000);
      const up = T0 + 7_000;
      if (end === "release") g.release(7, up);
      else g.releaseAll(up);
      expect(g.active).toBe(false);
      expect(decide(ask(up + TOUCH_MS - 1), at(g, up + TOUCH_MS - 1))).toBe("prompt");
      expect(decide(ask(up + TOUCH_MS + 1), at(g, up + TOUCH_MS + 1))).toBe("move");
    }
  });

  it("ignores hover moves and other pointers' releases; a wheel step counts", () => {
    const g = new Gesture();
    g.move(T0);
    expect(decide(ask(T0), at(g, T0))).toBe("move");
    g.press(1, T0);
    g.release(2, T0 + 9_000);
    expect(g.active).toBe(true);
    const w = new Gesture();
    w.wheel(T0);
    expect(decide(ask(T0 + 1_000), at(w, T0 + 1_000))).toBe("prompt");
  });

  it("control: an idle canvas lets the agent move the camera", () => {
    const g = new Gesture();
    expect(decide(ask(T0), at(g, T0))).toBe("move");
  });
});

describe("the activity strip's words", () => {
  const title = (id: string) => (id === A ? "Approval delays" : null);

  it("names who (or an agent), the action and the title, with the note apart", () => {
    const d = words(ev({ kind: "focus", node: A, hops: 2, note: "start here" }), title);
    expect(d).toEqual({ who: "an agent", action: "focused", target: "Approval delays, 2 hops", note: "start here", notShown: 0 });
    expect(words(ev({ kind: "clear" }, { by: { seat: "pij-xyz" } }), title).who).toBe("pij-xyz");
  });

  it("never names a node the view does not show, and counts it", () => {
    const d = words(ev({ kind: "focus", node: B, hops: 1 }), title);
    expect(d.target).toBe("a node that is not shown, 1 hop");
    expect(d.notShown).toBe(1);
    expect(words(ev({ kind: "highlight", nodes: [A, B, C], add: false }), title)).toMatchObject({ target: "3 nodes", notShown: 2 });
    expect(words(ev({ kind: "fit", nodes: [] }), title).target).toBe("the whole graph");
  });
});

describe("live removals (plan 127)", () => {
  it("take removed nodes out of the agent's highlight and focus, and leave the rest alone", () => {
    const s = { ...NO_AGENT, highlight: new Set([A, B]), focus: { node: A, hops: 2 } };
    expect(dropRemovedNodes(s, new Set([A]))).toMatchObject({ highlight: new Set([B]), focus: null });
    expect(dropRemovedNodes(s, new Set(["01ZZZZZZZZZZZZZZZZZZZZZZZZ"]))).toBe(s);
  });
});

describe("an agent's open (ws009 M1b1)", () => {
  const at = 1_000_000;
  const open = (graphs: string[], only = false, at_ms = at): AgentEvent => ({ seq: 9, at_ms, graph: graphs[0], by: null, command: { kind: "open", graphs, only, left_out: 0 } });
  it("adds the graphs not open at the end, so the open ones keep their places and colours", () => {
    expect(openPlan(open(["beta", "alpha", "gamma"]), ["alpha"], new Set(), at, 0)).toEqual({ kind: "add", add: ["beta", "gamma"], leftOut: 0, ask: false });
    expect(openPlan(open(["alpha"]), ["alpha"], new Set(), at, 0)).toEqual({ kind: "ignore" });
  });
  it("leaves out what passes 12 open, and counts it", () => {
    const eleven = Array.from({ length: 11 }, (_, i) => `g${i}`);
    expect(openPlan(open(["beta", "gamma", "delta"]), eleven, new Set(), at, 0)).toEqual({ kind: "add", add: ["beta"], leftOut: 2, ask: false });
  });
  it("asks for one sent before this view opened, and drops one older than 30 s", () => {
    expect(openPlan(open(["beta"], false, at - 1000), ["alpha"], new Set(), at, at - 500)).toMatchObject({ kind: "add", ask: true });
    expect(openPlan(open(["beta"], false, at - 31_000), ["alpha"], new Set(), at, 0)).toEqual({ kind: "ignore" });
  });
  it("plans --only as the graphs it names and the open ones it would close", () => {
    expect(openPlan(open(["beta"], true), ["alpha", "beta"], new Set(), at, 0)).toEqual({ kind: "only", graphs: ["beta"], closes: ["alpha"], leftOut: 0 });
    expect(openPlan(open(["beta"], true), ["beta"], new Set(), at, 0)).toEqual({ kind: "ignore" });
  });
  it("ignores an open naming a secured graph altogether (S3), even with --only and other graphs", () => {
    const secured = new Set(["vault"]);
    for (const e of [open(["vault"]), open(["beta", "vault"]), open(["vault", "beta"], true)]) {
      expect(openIgnored(e, secured)).toBe(true);
      expect(openPlan(e, ["alpha"], secured, at, 0)).toEqual({ kind: "ignore" });
    }
    expect(openIgnored(open(["beta"]), secured)).toBe(false);
  });
});

describe("opens that land one graph at a time (review B1, B2, B4)", () => {
  const at = 1_000_000;
  const open = (graphs: string[]): AgentEvent => ({ seq: 9, at_ms: at, graph: graphs[0], by: null, command: { kind: "open", graphs, only: false, left_out: 0 } });
  /** A read the test settles by hand. */
  const deferred = () => {
    let settle!: (v: string) => void;
    const p = new Promise<string>((r) => (settle = r));
    return { p, settle };
  };
  const tick = () => new Promise((r) => setTimeout(r, 0));

  it("folds two opens against the set the view is heading to, in order", () => {
    let intended = ["alpha"];
    for (const e of [open(["beta"]), open(["gamma"])]) {
      const plan = openPlan(e, intended, new Set(), at, 0);
      if (plan.kind === "add") intended = [...intended, ...plan.add];
    }
    expect(intended).toEqual(["alpha", "beta", "gamma"]);
  });

  it("lets a graph that arrives join at once while a slow one loads, and a superseded read never joins", async () => {
    const reads = new Map<string, ReturnType<typeof deferred>[]>();
    const read = (a: string) => {
      const d = deferred();
      reads.set(a, [...(reads.get(a) ?? []), d]);
      return d.p;
    };
    let onScreen = ["alpha"];
    // The view's loading effect, as GraphApp runs it: each arrival joins against the set it was started for.
    const run = (intended: string[]) => {
      let alive = true;
      const arrived: string[] = [];
      const add = intended.filter((a) => !onScreen.includes(a));
      void readEach(add, read, () => alive, (a) => {
        arrived.push(a);
        onScreen = openLanded(intended, onScreen, arrived);
      }, () => {});
      return () => (alive = false);
    };
    // The first open's beta is slow; the second open (beta and gamma) starts while it loads, superseding it.
    const stopFirst = run(["alpha", "beta"]);
    stopFirst();
    const stopSecond = run(["alpha", "beta", "gamma"]);
    reads.get("gamma")![0].settle("gamma");
    await tick();
    expect(onScreen).toEqual(["alpha", "gamma"]);
    reads.get("beta")![1].settle("beta");
    await tick();
    expect(onScreen).toEqual(["alpha", "beta", "gamma"]);
    // A person's own pick replaces the set; the first run's beta settling late changes nothing.
    stopSecond();
    onScreen = ["alpha"];
    reads.get("beta")![0].settle("beta");
    await tick();
    expect(onScreen).toEqual(["alpha"]);
  });

  it("moves the camera by itself only if still allowed when the move would happen", () => {
    const idle = { now: 10_000, follow: true, visible: true, busy: false, lastTouch: 10_000 - TOUCH_MS };
    expect(openMayMove(idle)).toBe(true);
    expect(openMayMove({ ...idle, busy: true })).toBe(false);
    expect(openMayMove({ ...idle, lastTouch: idle.now - 100 })).toBe(false);
    expect(openMayMove({ ...idle, follow: false })).toBe(false);
    expect(openMayMove({ ...idle, visible: false })).toBe(false);
  });

  it("says the daemon's refused graphs and the view's capped ones alike", () => {
    expect(openTail(0, 0)).toBe("");
    expect(openTail(1, 0)).toBe(" · 1 graph left out");
    expect(openTail(2, 1)).toBe(" · 2 graphs left out · 1 left out (12 is the most)");
  });
});

describe("an agent's raise (ws009 M1b2)", () => {
  const at = 1_000_000;
  const open = (graphs: string[], raise: boolean, at_ms = at): AgentEvent => ({ seq: 9, at_ms, graph: graphs[0], by: null, command: { kind: "open", graphs, only: false, left_out: 0, raise } });
  it("titles the tab only for a fresh raise of graphs the view may show", () => {
    expect(raiseTitle(open(["alpha", "beta"], true), new Set(), at)).toBe("\u25cf umx: agent wants to show you alpha, beta");
    expect(raiseTitle(open(["alpha"], false), new Set(), at)).toBeNull();
    expect(raiseTitle(open(["vault"], true), new Set(["vault"]), at)).toBeNull();
    expect(raiseTitle(open(["alpha"], true, at - STALE_MS - 1), new Set(), at)).toBeNull();
  });
});

describe("the tab's raise (ws009 M1b2)", () => {
  function page(focused: boolean) {
    const on = new Map<string, Set<() => void>>();
    const listen = {
      addEventListener: (t: string, f: () => void) => on.set(t, (on.get(t) ?? new Set()).add(f)),
      removeEventListener: (t: string, f: () => void) => on.get(t)?.delete(f),
    };
    const doc = { ...listen, title: "umx graph view", visibilityState: focused ? "visible" : "hidden", hasFocus: () => focused };
    let focusAsked = 0;
    const win = { ...listen, focus: () => void focusAsked++ };
    const fire = (t: string) => [...(on.get(t) ?? [])].forEach((f) => f());
    return { doc, win, fire, asked: () => focusAsked, listeners: () => [...on.values()].reduce((n, s) => n + s.size, 0) };
  }
  it("marks the title and asks for focus, and puts the title back once the person looks", () => {
    const p = page(false);
    const r = new Raiser(p.doc, p.win, () => undefined);
    r.raise("\u25cf umx: agent wants to show you beta");
    r.raise("\u25cf umx: agent wants to show you gamma");
    expect(p.doc.title).toBe("\u25cf umx: agent wants to show you gamma");
    expect(p.asked()).toBe(2);
    p.fire("focus");
    expect(p.doc.title).toBe("umx graph view");
    expect(p.listeners()).toBe(0);
  });
  it("in a tab the person is looking at, puts the title back after a while", () => {
    const p = page(true);
    let later: (() => void) | null = null;
    const r = new Raiser(p.doc, p.win, (f) => {
      later = f;
    });
    r.raise("\u25cf umx: agent wants to show you beta");
    expect(p.doc.title).toContain("beta");
    (later as unknown as () => void)();
    expect(p.doc.title).toBe("umx graph view");
  });
});


describe("an agent centres the view (PRD 005 centre-11)", () => {
  const now = 100_000;
  it("centres on the newest fresh focus --centre, and the strip says so", () => {
    const events = [qev({ kind: "focus", node: A, hops: 1, centre: true }, { at_ms: now }), qev({ kind: "focus", node: B, hops: 2, centre: true }, { at_ms: now })];
    expect(agentCentre(events, null, now)).toEqual({ key: qB, by: "agent" });
    expect(words(ev({ kind: "focus", node: A, hops: 2, centre: true }), () => "Approval delays").action).toBe("centred the view on");
    // The rings' own fit frames them: no jump to the node.
    expect(cameraAsk(ev({ kind: "focus", node: A, hops: 1, centre: true }))).toBeNull();
  });

  it("never centres for a plain focus or a stale one, and leaves the centring as it was", () => {
    const person = { key: qC, by: "person" as const };
    expect(agentCentre([qev({ kind: "focus", node: A, hops: 1 }, { at_ms: now })], person, now)).toBe(person);
    expect(agentCentre([qev({ kind: "focus", node: A, hops: 1, centre: true }, { at_ms: now - STALE_MS - 1 })], null, now)).toBeNull();
  });

  it("clear in the centre's graph ends an agent's centring, never a person's or another graph's", () => {
    const clear = (graph: string) => qev({ kind: "clear" }, { graph, at_ms: now });
    expect(agentCentre([clear("demo")], { key: qA, by: "agent" }, now)).toBeNull();
    const agentElsewhere = { key: qA, by: "agent" as const };
    expect(agentCentre([clear("other")], agentElsewhere, now)).toBe(agentElsewhere);
    const person = { key: qA, by: "person" as const };
    expect(agentCentre([clear("demo")], person, now)).toBe(person);
  });
});

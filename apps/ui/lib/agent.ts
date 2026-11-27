// What agents set on the view (ws009 S6b, plan 114), as pure functions: the state their events build, how an agent
// filter lands on the chips and comes off again, when the camera may move, and the activity strip's words. Agent
// commands change presentation only. They never touch the provenance and retired toggles, never open a graph, and
// never reveal a hidden node: a node the view does not show is named as "not shown", never by its title.
import { type FilterDim, type Filters, NO_FILTERS } from "@/lib/graph/filters";
import { aliasOfKey, MAX_OPEN, qid } from "@/lib/graph/keys";
import type { AgentActor, AgentEvent, UiCommand } from "@/lib/umx/socket";

/** Focus and fit older than this when they reach the view never move the camera (no replayed jumps). */
export const STALE_MS = 30_000;
/** The camera never moves for an agent within this long of the person touching the canvas. */
export const TOUCH_MS = 5_000;
export const HISTORY_MAX = 100;

/**
 * What agents set on the open graphs (plan 133: one state for every graph on the canvas). Node ids are graph-qualified
 * keys (lib/graph/keys); each command acts within the graph it named: a highlight replaces only that graph's part of
 * the highlight, and clear takes off only what was set in that graph.
 */
export type AgentState = {
  /** The agent's highlight: a halo, nothing dimmed. */
  highlight: ReadonlySet<string>;
  /** The agent's focus: everything further than `hops` from `node` is dimmed. The newest focus in any open graph. */
  focus: { node: string; hops: number } | null;
  /** The event that set the focus, and its note. */
  focusMark: Mark | null;
  /** The event that put each highlighted node in the highlight, and its note: what activeNotes reads. */
  marks: ReadonlyMap<string, Mark>;
  /** The agent's filter, as sent, and the graph that sent it: it lives on the chips (each one replaces the last; Undo,
   * clear in its graph or an empty filter takes it off again). */
  filter: { seq: number; show: string[]; hide: string[]; graph?: string } | null;
  /** Newest first. */
  history: AgentEvent[];
};

export type Mark = { seq: number; note: string | null };

export const NO_AGENT: AgentState = { highlight: new Set(), focus: null, filter: null, history: [], marks: new Map(), focusMark: null };

/** The event with each node id it names qualified by the event's graph, as the canvas keys them. */
export function qualifyEvent(e: AgentEvent): AgentEvent {
  const c = e.command;
  const q = (id: string) => qid(e.graph, id);
  switch (c.kind) {
    case "focus":
      return { ...e, command: { ...c, node: q(c.node) } };
    case "highlight":
    case "fit":
      return { ...e, command: { ...c, nodes: c.nodes.map(q) } };
    default:
      return e;
  }
}

/** Applies one qualified event (qualifyEvent). */
export function reduce(s: AgentState, e: AgentEvent): AgentState {
  const history = [e, ...s.history].slice(0, HISTORY_MAX);
  const c = e.command;
  const elsewhere = [...s.highlight].filter((k) => aliasOfKey(k) !== e.graph);
  switch (c.kind) {
    case "focus":
      return { ...s, history, focus: { node: c.node, hops: c.hops }, focusMark: { seq: e.seq, note: c.note ?? null } };
    case "highlight": {
      // A highlight replaces only its own graph's part (plan 133), and its notes with it.
      const marks = new Map(c.add ? s.marks : [...s.marks].filter(([k]) => aliasOfKey(k) !== e.graph));
      for (const id of c.nodes) marks.set(id, { seq: e.seq, note: c.note ?? null });
      return { ...s, history, highlight: new Set(c.add ? [...s.highlight, ...c.nodes] : [...elsewhere, ...c.nodes]), marks };
    }
    case "filter":
      return { ...s, history, filter: { seq: e.seq, show: c.show, hide: c.hide, graph: e.graph } };
    case "fit":
    case "open":
      return { ...s, history };
    case "clear":
      // What agents set in this graph (umx docs get ui): highlight, focus and filter, and their notes; never the
      // person's selection, focus or toggles, nor what was set in another open graph. The view takes the filter's chip
      // changes back off (undoAgentFilter).
      {
        const focusStays = s.focus !== null && aliasOfKey(s.focus.node) !== e.graph;
        return {
          ...s,
          history,
          highlight: new Set(elsewhere),
          marks: new Map([...s.marks].filter(([k]) => aliasOfKey(k) !== e.graph)),
          focus: focusStays ? s.focus : null,
          focusMark: focusStays ? s.focusMark : null,
          filter: s.filter && s.filter.graph !== undefined && s.filter.graph !== e.graph ? s.filter : null,
        };
      }
  }
}

/**
 * The agent's notes now on screen, by node (K1 callouts): from the highlight and focus that are active, never from
 * history. A node named by both takes the newer one's note, or none when the newer one had none; clear, a replacing
 * highlight or another focus takes a note away (review B2).
 */
export function activeNotes(s: AgentState): Map<string, string> {
  const out = new Map<string, string>();
  for (const id of s.highlight) {
    const mark = s.marks.get(id);
    if (mark?.note != null) out.set(id, mark.note);
  }
  const f = s.focus;
  const fm = s.focusMark;
  if (f && fm && fm.seq >= (s.highlight.has(f.node) ? (s.marks.get(f.node)?.seq ?? -1) : -1)) {
    if (fm.note !== null) out.set(f.node, fm.note);
    else out.delete(f.node);
  }
  return out;
}

/** Nodes a live change removed leave the agent's highlight and focus (plan 127); the history keeps its words. */
export function dropRemovedNodes(s: AgentState, removed: ReadonlySet<string>): AgentState {
  const highlight = [...s.highlight].filter((id) => !removed.has(id));
  const focusGone = s.focus !== null && removed.has(s.focus.node);
  if (highlight.length === s.highlight.size && !focusGone) return s;
  const marks = new Map([...s.marks].filter(([id]) => !removed.has(id)));
  return { ...s, highlight: new Set(highlight), focus: focusGone ? null : s.focus, focusMark: focusGone ? null : s.focusMark, marks };
}

/** A centring (PRD 005) and who made it. */
export type Centring = { key: string; by: "person" | "agent" };

/**
 * What a batch of qualified events does to the view's centring (PRD 005 centre-11): the newest `focus --centre` no
 * older than STALE_MS centres it on that node (a replayed or old one never re-arranges the view); `clear` in a graph
 * ends an agent's centring on a node of that graph, never a person's. Returns `current` itself when nothing changes.
 */
export function agentCentre<T extends Centring>(events: readonly AgentEvent[], current: T | null, now: number): T | Centring | null {
  let out: T | Centring | null = current;
  for (const e of events) {
    const c = e.command;
    if (c.kind === "focus" && c.centre && now - e.at_ms <= STALE_MS) out = { key: c.node, by: "agent" };
    else if (c.kind === "clear" && out?.by === "agent" && aliasOfKey(out.key) === e.graph) out = null;
  }
  return out;
}

/** Where the view's actions on events have got to, per daemon epoch: an event at or below is never acted on again. */
export type Applied = { epoch: string; seq: number } | null;

export type Reply = { epoch: string; seq: number; reset: boolean; events: AgentEvent[] };

/**
 * A reply from /api/events: the new state, and the events still to act on (filters on the chips, camera moves), each
 * with its node ids qualified by its graph. A reset rebuilds the state from every kept event, but acts only on those
 * after what was already acted on in the same epoch, so a reset never replays a filter or a camera move.
 */
export function receive(state: AgentState, applied: Applied, reply: Reply): { state: AgentState; act: AgentEvent[]; applied: Applied } {
  const events = reply.events.map(qualifyEvent);
  const next = events.reduce(reduce, reply.reset ? NO_AGENT : state);
  const after = applied !== null && applied.epoch === reply.epoch ? applied.seq : -1;
  const act = events.filter((e) => e.seq > after);
  const top = act.reduce((m, e) => Math.max(m, e.seq), after);
  return { state: next, act, applied: { epoch: reply.epoch, seq: top } };
}

const DIMS: Record<string, FilterDim> = { relation: "relations", type: "types", status: "statuses" };

/** `relation:<r>`, `type:<t>` or `status:<s>` as a chip; anything else is not one. */
export function chipOf(name: string): { dim: FilterDim; key: string } | null {
  const i = name.indexOf(":");
  const dim = DIMS[name.slice(0, i)];
  return i > 0 && dim && i < name.length - 1 ? { dim, key: name.slice(i + 1) } : null;
}

/** One chip an agent filter changed: off before and after. */
export type ChipChange = { dim: FilterDim; key: string; wasOff: boolean; off: boolean };

/** Turns the named chips off (hide) or on (show), hide winning a name in both. Only chips: the toggles stay. */
export function applyAgentFilter(f: Filters, cmd: { show: string[]; hide: string[] }): { filters: Filters; changes: ChipChange[] } {
  const sets: Record<FilterDim, Set<string>> = { relations: new Set(f.relations), types: new Set(f.types), statuses: new Set(f.statuses) };
  const want = new Map<string, { dim: FilterDim; key: string; off: boolean }>();
  for (const [names, off] of [[cmd.show, false], [cmd.hide, true]] as const) {
    for (const name of names) {
      const chip = chipOf(name);
      if (chip) want.set(`${chip.dim}:${chip.key}`, { ...chip, off });
    }
  }
  const changes: ChipChange[] = [];
  for (const { dim, key, off } of want.values()) {
    const wasOff = sets[dim].has(key);
    if (wasOff === off) continue;
    if (off) sets[dim].add(key);
    else sets[dim].delete(key);
    changes.push({ dim, key, wasOff, off });
  }
  return { filters: sets, changes };
}

/**
 * Puts back what an agent filter changed, chip by chip, and touches no other chip. A chip the person has flipped back
 * since already holds its old state (a chip is on or off), so it stays as the person set it.
 */
export function undoAgentFilter(f: Filters, changes: ChipChange[]): Filters {
  const sets: Record<FilterDim, Set<string>> = { relations: new Set(f.relations), types: new Set(f.types), statuses: new Set(f.statuses) };
  for (const c of changes) {
    if (c.wasOff) sets[c.dim].add(c.key);
    else sets[c.dim].delete(c.key);
  }
  return sets;
}

/**
 * The chips, and which of their changes still belong to the agent (review R224-3). An agent filter, its Undo, clear
 * and a reset to no filter move only the agent's chips; a person's toggle, Show or Reset filters takes the chips it
 * touches over, so nothing an agent does later turns them back.
 */
export type Ledger = {
  filters: Filters;
  /** The agent's chip changes still in force, each as its filter made it. */
  agent: ChipChange[];
  /** The seq of the agent filter those changes came from, or null. */
  seq: number | null;
};

export const LEDGER0: Ledger = { filters: NO_FILTERS, agent: [], seq: null };

const chipKey = (c: { dim: FilterDim; key: string }) => `${c.dim}:${c.key}`;

/** An agent filter: replaces the agent's last one (taking its chips back off first), then sets the named chips. */
export function agentFilter(l: Ledger, seq: number, cmd: { show: string[]; hide: string[] }): Ledger {
  const r = applyAgentFilter(undoAgentFilter(l.filters, l.agent), cmd);
  return { filters: r.filters, agent: r.changes, seq };
}

/** Undo, clear, or a reset to no filter: the agent's chips go back; the person's stay as they are. */
export function agentOff(l: Ledger): Ledger {
  return { filters: undoAgentFilter(l.filters, l.agent), agent: [], seq: null };
}

/** The chips that differ between two filter states, as "dim:key". */
export function chipDiff(a: Filters, b: Filters): Set<string> {
  const out = new Set<string>();
  for (const dim of ["relations", "types", "statuses"] as const) {
    for (const k of a[dim]) if (!b[dim].has(k)) out.add(`${dim}:${k}`);
    for (const k of b[dim]) if (!a[dim].has(k)) out.add(`${dim}:${k}`);
  }
  return out;
}

/**
 * A person sets the chips: they own every chip in `owned` from now on ("all" for Reset filters; by default the chips
 * that changed). The agent keeps only its changes to chips nobody has touched since.
 */
export function personSets(l: Ledger, filters: Filters, owned: "all" | Iterable<string> = chipDiff(l.filters, filters)): Ledger {
  if (owned === "all") return { filters, agent: [], seq: l.seq };
  const taken = new Set(owned);
  return { filters, agent: l.agent.filter((c) => !taken.has(chipKey(c))), seq: l.seq };
}

/**
 * The open graphs changed (plan 150 review B2): the relation chips the person turned off stay off, as their
 * quiet or normal choice does; the agent's chip changes and the type and status chips start over.
 */
export function keepPersonRelations(l: Ledger): Ledger {
  return { ...LEDGER0, filters: { ...NO_FILTERS, relations: new Set(undoAgentFilter(l.filters, l.agent).relations) } };
}

/** A camera move an agent asked for in `graph`: focus (one node) or fit (some nodes, or none for that whole graph). */
export type CameraAsk = { seq: number; at_ms: number; kind: "focus" | "fit"; nodes: string[]; graph: string };

export function cameraAsk(e: AgentEvent): CameraAsk | null {
  const c = e.command;
  // A centring fits the camera to its rings (PRD 005 centre-10); jumping to the node would hide them.
  if (c.kind === "focus" && c.centre) return null;
  if (c.kind === "focus") return { seq: e.seq, at_ms: e.at_ms, kind: "focus", nodes: [c.node], graph: e.graph };
  if (c.kind === "fit") return { seq: e.seq, at_ms: e.at_ms, kind: "fit", nodes: c.nodes, graph: e.graph };
  return null;
}

/**
 * The nodes a fit moves the camera to (review B5): the ones it named; with none, every shown node of its own
 * graph. With one graph open that is the whole canvas, so it stays the plain reset (an empty list). With several open
 * and none of its graph's nodes shown, null: the camera stays where it is, never fitting the other graphs (round 2).
 */
export function fitTargets(ask: CameraAsk, nodes: Iterable<{ key: string; graph: string; hidden: boolean }>, open: number): string[] | null {
  if (ask.nodes.length > 0 || open < 2) return ask.nodes;
  const out: string[] = [];
  for (const n of nodes) if (n.graph === ask.graph && !n.hidden) out.push(n.key);
  return out.length > 0 ? out : null;
}

/** What the view does for a camera request: jump to a node, fit some nodes (an empty list resets to the whole
 * canvas), or nothing. Prompt Go, replay and follow mode all go through this. */
export type CameraMove = { kind: "jump"; id: string } | { kind: "fit"; nodes: string[] } | null;

export function cameraMove(ask: CameraAsk, nodes: Iterable<{ key: string; graph: string; hidden: boolean }>, open: number): CameraMove {
  if (ask.kind === "focus") return { kind: "jump", id: ask.nodes[0] };
  const targets = fitTargets(ask, nodes, open);
  return targets === null ? null : { kind: "fit", nodes: targets };
}

/** The canvas's part of a fit: an empty list resets the camera to the whole canvas; otherwise the shown nodes named,
 * and with none of them shown the camera stays. */
export function canvasFit(nodes: readonly string[], shown: (id: string) => boolean): "reset" | "stay" | string[] {
  if (nodes.length === 0) return "reset";
  const ids = nodes.filter(shown);
  return ids.length > 0 ? ids : "stay";
}

/** What a batch of agent events does to the chips and the camera, each command within its own graph. */
export type BatchIn = { ledger: Ledger; filterFrom: string | null; pending: CameraAsk | null; prompt: CameraAsk | null };
export type BatchOut = BatchIn & { filterTouched: boolean; chipsBefore: Filters };

/**
 * Takes one batch (review B5): a filter replaces the agent filter on the chips and remembers its graph; clear
 * takes that filter off only when it came from the same graph, and drops only camera requests (waiting, prompting or
 * earlier in the batch) of its own graph. The newest camera request waits for its decision; a new one replaces the
 * prompt.
 */
export function takeBatch(events: readonly AgentEvent[], s: BatchIn): BatchOut {
  let { ledger, filterFrom, pending, prompt } = s;
  let filterTouched = false;
  let ask: CameraAsk | null = null;
  const mine = (a: CameraAsk | null, g: string) => (a && a.graph === g ? null : a);
  for (const e of events) {
    if (e.command.kind === "filter") {
      ledger = agentFilter(ledger, e.seq, e.command);
      filterFrom = e.graph;
      filterTouched = true;
    } else if (e.command.kind === "clear") {
      if (filterFrom === null || filterFrom === e.graph) {
        ledger = agentOff(ledger);
        filterFrom = null;
        filterTouched = true;
      }
      ask = mine(ask, e.graph);
      pending = mine(pending, e.graph);
      prompt = mine(prompt, e.graph);
    }
    ask = cameraAsk(e) ?? ask;
  }
  if (ask) {
    pending = ask;
    prompt = null;
  }
  return { ledger, filterFrom, pending, prompt, filterTouched, chipsBefore: s.ledger.filters };
}

/**
 * What to do with a camera ask now: move; prompt ("Agent wants to show X", Go); defer until the tab is visible; drop
 * a stale one; or not_shown when none of its nodes is on the canvas (never revealed).
 */
export type Decision = "move" | "prompt" | "defer" | "drop" | "not_shown";

export function decide(
  ask: CameraAsk,
  at: { now: number; follow: boolean; visible: boolean; lastTouch: number; busy: boolean; shown: (id: string) => boolean },
): Decision {
  if (at.now - ask.at_ms > STALE_MS) return "drop";
  if (ask.nodes.length > 0 && !ask.nodes.some(at.shown)) return "not_shown";
  if (!at.visible) return "defer";
  if (!at.follow || at.busy || at.now - at.lastTouch < TOUCH_MS) return "prompt";
  return "move";
}

/**
 * The person's use of the canvas (review R224-4): a gesture lasts from a press until its release or cancel, however
 * long, and every move during it, every wheel step and the release itself count as touching. A pointer moving with
 * nothing pressed (a hover) is not use. An agent never moves the camera while a gesture is active, nor within
 * TOUCH_MS of its end.
 */
export class Gesture {
  private pressed = new Set<number>();
  private last = Number.NEGATIVE_INFINITY;
  press(id: number, now: number) {
    this.pressed.add(id);
    this.last = now;
  }
  move(now: number) {
    if (this.pressed.size > 0) this.last = now;
  }
  release(id: number, now: number) {
    if (this.pressed.delete(id)) this.last = now;
  }
  /** Every press ends (the window lost focus, so no release will come). */
  releaseAll(now: number) {
    if (this.pressed.size > 0) this.last = now;
    this.pressed.clear();
  }
  wheel(now: number) {
    this.last = now;
  }
  get active(): boolean {
    return this.pressed.size > 0;
  }
  get lastTouch(): number {
    return this.last;
  }
}

/** Who did it, as the event claims (never verified): its seat, role or user, else "an agent". */
export function who(by: AgentActor | null): string {
  return by?.seat || by?.role || by?.user || "an agent";
}

/** An event in words. `title` gives a node's title only when the view shows it; null otherwise. */
export type Described = { who: string; action: string; target: string; note: string | null; notShown: number };

export function describe(e: AgentEvent, title: (id: string) => string | null): Described {
  const c: UiCommand = e.command;
  const nodes = c.kind === "focus" ? [c.node] : c.kind === "highlight" || c.kind === "fit" ? c.nodes : [];
  const titles = nodes.map(title);
  const notShown = titles.filter((t) => t === null).length;
  const shown = titles.filter((t): t is string => t !== null);
  const many = (n: number) => `${n} node${n === 1 ? "" : "s"}`;
  const target = () => (nodes.length === 1 ? (shown[0] ?? "a node that is not shown") : many(nodes.length));
  const base = { who: who(e.by), note: c.note ?? null, notShown };
  switch (c.kind) {
    case "focus":
      return { ...base, action: c.centre ? "centred the view on" : "focused", target: `${target()}, ${c.hops} hop${c.hops === 1 ? "" : "s"}` };
    case "highlight":
      return { ...base, action: c.add ? "added to its highlight" : "highlighted", target: target() };
    case "fit":
      return { ...base, action: "fitted the view to", target: nodes.length === 0 ? "the whole graph" : target() };
    case "filter": {
      const parts = [c.hide.length ? `hid ${c.hide.join(", ")}` : "", c.show.length ? `showed ${c.show.join(", ")}` : ""].filter(Boolean);
      return { ...base, action: "changed the filters", target: parts.join("; ") || "back to none" };
    }
    case "clear":
      return { ...base, action: "cleared what agents set", target: "" };
    case "open":
      return { ...base, action: c.only ? "asked to show only" : "opened", target: c.graphs.join(", ") };
  }
}

/**
 * Whether the view ignores an agent's open altogether (ws009 M1b1, security S3): it names a graph the view knows is
 * secured. The daemon never sends one (S2); if one arrives anyway, nothing acts on it and nothing names it, not even
 * the strip.
 */
export function openIgnored(e: AgentEvent, secured: ReadonlySet<string>): boolean {
  return e.command.kind === "open" && e.command.graphs.some((g) => secured.has(g));
}

/**
 * The title a tab shows for an agent's `umx ui open --raise` (ws009 M1b2, ui-expert 2026-11-12), or null: not a raise,
 * ignored (S3), or stale. The tab also asks for focus, which a browser may refuse; the title still shows.
 */
export function raiseTitle(e: AgentEvent, secured: ReadonlySet<string>, now: number): string | null {
  const c = e.command;
  if (c.kind !== "open" || !c.raise || openIgnored(e, secured) || now - e.at_ms > STALE_MS) return null;
  return `\u25cf umx: agent wants to show you ${c.graphs.join(", ")}`;
}

/** What the view does with an agent's open (ws009 M1b1, ui-expert design 2026-11-12). */
export type OpenPlan =
  | { kind: "ignore" }
  /** Added at the end of the open set (colours stay); `leftOut` past the 12 cap; `ask` when it came before this view
   * opened, so it is offered, never applied silently. */
  | { kind: "add"; add: string[]; leftOut: number; ask: boolean }
  /** In place of the open set: always asked first. `closes` are the open graphs it would take away. */
  | { kind: "only"; graphs: string[]; closes: string[]; leftOut: number };

/** An open event's plan against the graphs open now. `viewSince`: when this view opened (Date.now()). */
export function openPlan(e: AgentEvent, open: readonly string[], secured: ReadonlySet<string>, now: number, viewSince: number): OpenPlan {
  const c = e.command;
  if (c.kind !== "open" || openIgnored(e, secured) || now - e.at_ms > STALE_MS) return { kind: "ignore" };
  if (c.only) {
    const graphs = c.graphs.slice(0, MAX_OPEN);
    const closes = open.filter((a) => !graphs.includes(a));
    if (closes.length === 0 && graphs.every((g) => open.includes(g))) return { kind: "ignore" };
    return { kind: "only", graphs, closes, leftOut: c.graphs.length - graphs.length };
  }
  const fresh = c.graphs.filter((g) => !open.includes(g));
  const room = Math.max(0, MAX_OPEN - open.length);
  const add = fresh.slice(0, room);
  if (fresh.length === 0) return { kind: "ignore" };
  return { kind: "add", add, leftOut: fresh.length - add.length, ask: e.at_ms < viewSince };
}

/** What the strip adds after an open's graphs: the daemon's refused ones, counted and never named, and those past the
 * view's 12 (ui-expert N2: the two read alike). */
export function openTail(refused: number, capped: number): string {
  const parts = [
    refused > 0 ? `${refused} graph${refused === 1 ? "" : "s"} left out` : "",
    capped > 0 ? `${capped} left out (${MAX_OPEN} is the most)` : "",
  ].filter(Boolean);
  return parts.map((p) => ` · ${p}`).join("");
}

/**
 * Whether an agent's open may still move the camera by itself, asked when the move would happen (review B2):
 * following, the tab visible, no gesture and nothing touched for TOUCH_MS. Otherwise the strip offers Go.
 */
export function openMayMove(at: { now: number; follow: boolean; visible: boolean; busy: boolean; lastTouch: number }): boolean {
  return at.follow && at.visible && !at.busy && at.now - at.lastTouch >= TOUCH_MS;
}

/**
 * The open set after `landed` arrives (review B4): the intended graphs, in their order, that are on screen or
 * have arrived; a graph no longer intended is left out, so a read that settles late never brings one back.
 */
export function openLanded(intended: readonly string[], onScreen: readonly string[], arrived: readonly string[]): string[] {
  return intended.filter((a) => onScreen.includes(a) || arrived.includes(a));
}

/**
 * Reads each graph on its own, all at once (review B4): each one that arrives goes to `land` straight away,
 * one that fails to `failed`, while `alive()`; a slow graph holds back no other. Settles when all have.
 */
export async function readEach<T>(aliases: readonly string[], read: (alias: string) => Promise<T>, alive: () => boolean, land: (alias: string, got: T) => void, failed: (alias: string, e: unknown) => void): Promise<void> {
  await Promise.all(
    aliases.map(async (a) => {
      let got: T;
      try {
        got = await read(a);
      } catch (e) {
        if (alive()) failed(a, e);
        return;
      }
      if (alive()) land(a, got);
    }),
  );
}

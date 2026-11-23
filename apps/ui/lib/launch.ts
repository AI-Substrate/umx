// What a launch link asks for (`umx ui --graph`, an agent's raise). The fragment is #t=<code>&g=<alias>[,<alias>...],
// where <code> is a single-use launch code umx armed on the server's stdin; it never reaches the server's logs. A
// plain `umx ui` opens the bare URL, with no fragment. Several aliases open several graphs on one canvas (plan 133). A
// link opened in a tab that is already open only changes the fragment, so the view also reads it on `hashchange`
// (plan 113).
import { MAX_OPEN } from "@/lib/graph/keys";
import type { GraphSummary } from "@/lib/types";

const CODE = /(?:^#|&)t=([A-Za-z0-9_-]{43})(?:&|$)/;

/** The graphs `umx ui --graph <a,b,...>` named in the fragment, each once and at most MAX_OPEN, or null. */
export function requestedGraphs(hash: string): string[] | null {
  const match = /(?:^#|&)g=([^&]{1,1024})(?:&|$)/.exec(hash);
  if (!match) return null;
  let text: string;
  try {
    text = decodeURIComponent(match[1]);
  } catch {
    return null;
  }
  const list = [...new Set(text.split(",").filter((a) => a !== ""))].slice(0, MAX_OPEN);
  return list.length > 0 ? list : null;
}

/** What opens first, and what of the named graphs could not. */
export type FirstPick = {
  /** The graphs to open: the named ones the picker would open, else the first loaded graph that is not secured. */
  aliases: string[];
  /** True when `aliases` are the named graphs (a fresh launch then switches to them). */
  named: boolean;
  /** Named graphs that are not listed. */
  missing: string[];
  /** Named graphs that are listed but cannot be opened. */
  blocked: GraphSummary[];
};

/**
 * The graphs to open first: the named ones the picker would open (a secured graph opens only this way, or by a pick),
 * else the first loaded graph that is not secured. The view never picks an idle graph by itself, since opening it
 * loads it.
 */
export function firstGraphs(list: GraphSummary[], named: readonly string[] | null): FirstPick {
  const auto = list.find((g) => g.state === "loaded" && g.openable && !g.secured)?.alias;
  const fallback = auto ? [auto] : [];
  if (named === null) return { aliases: fallback, named: false, missing: [], blocked: [] };
  const out: FirstPick = { aliases: [], named: true, missing: [], blocked: [] };
  for (const alias of named) {
    const rows = list.filter((g) => g.alias === alias);
    const row = rows.find((g) => g.state === "loaded") ?? rows[0];
    if (!row) out.missing.push(alias);
    else if (!row.openable) out.blocked.push(row);
    else out.aliases.push(alias);
  }
  return out.aliases.length > 0 ? out : { ...out, aliases: fallback, named: false };
}

/** What a launch link asked for, and its number: only the latest launch may change the view (review R222-2). */
export type Launch = { named: string[] | null; gen: number };

/**
 * Numbers the launches in a tab. Each link with a code takes the next number when it lands; a result from any
 * other number is obsolete. A read that is not a launch (a background re-list) carries `now()` and goes obsolete when
 * a launch begins.
 */
export class Launches {
  private gen = 0;
  begin(): number {
    return ++this.gen;
  }
  now(): number {
    return this.gen;
  }
  latest(gen: number): boolean {
    return gen === this.gen;
  }
}

export type LaunchDeps = {
  location: { hash: string; pathname: string; search: string };
  history: { replaceState(data: unknown, unused: string, url: string): void };
  /** POST /api/launch with the code (same origin; the proxy checks Host and Origin); true when it was taken. */
  post: (code: string) => Promise<boolean>;
};

/**
 * Reads the launch fragment, drops it from the address bar (replaceState fires no hashchange), and hands its code to
 * the server. Null when the fragment holds no code. The named graphs count only with a code this run took, in the
 * same fragment (security N1): any page can link to the view's port with `#g=<alias>`, and opening a named graph may
 * open a secured one or load an idle one, so a lone g= opens and loads nothing (and does not cancel a launch in
 * flight). A code the server refuses (used, stale, made up) names no graph.
 */
export async function takeLaunch(deps: LaunchDeps, launches: Launches): Promise<Launch | null> {
  const hash = deps.location.hash;
  const code = CODE.exec(hash)?.[1] ?? null;
  const named = requestedGraphs(hash);
  if (hash !== "") deps.history.replaceState(null, "", deps.location.pathname + deps.location.search);
  if (code === null) return null;
  const gen = launches.begin();
  let taken = false;
  try {
    taken = await deps.post(code);
  } catch {
    // Not taken: the view lists the graphs and opens none it was not asked to by umx.
  }
  return { named: taken ? named : null, gen };
}

/**
 * Reads the fragment again whenever a new launch link lands in this tab. `onLaunch` runs only for the latest launch and
 * never after the returned unsubscribe.
 */
export function watchLaunch(target: EventTarget, deps: LaunchDeps, launches: Launches, onLaunch: (launch: Launch) => void): () => void {
  let live = true;
  const listener = () => {
    void takeLaunch(deps, launches).then((launch) => {
      if (live && launch && launches.latest(launch.gen)) onLaunch(launch);
    });
  };
  target.addEventListener("hashchange", listener);
  return () => {
    live = false;
    target.removeEventListener("hashchange", listener);
  };
}

/** A launch link's graph the index did not list, and the live alias umx resolved it to (plan 215). */
export type Followed = { from: string; to: string; id: string };

/** The graph index, and for a launch: what its unlisted names now are (`renamed`) or why umx refused them. */
export type Listed = { graphs: GraphSummary[]; renamed?: Followed[]; refused?: { alias: string; issue: { code: string; message: string; next?: string | null } }[] };

/** A launch's names with each former alias read as its live alias, each once (plan 215). Only the names the link gave
 * are mapped: nothing else is followed. */
export function followedNames(named: readonly string[], renamed: readonly Followed[]): string[] {
  return [...new Set(named.map((a) => renamed.find((r) => r.from === a)?.to ?? a))];
}

/** What a graph-list read for a launch tells the view. */
export type ListResult =
  | { kind: "graphs"; graphs: GraphSummary[]; pick: FirstPick; named: string[] | null; fresh: boolean; renamed: Followed[]; refused: NonNullable<Listed["refused"]> }
  | { kind: "failed"; error: unknown };

/**
 * Lists the graphs for launch `gen` (or a background re-list carrying `launches.now()`) and hands the result to
 * `apply` only if no newer launch began meanwhile and `alive()` still holds: an obsolete list or failure changes
 * nothing (review R222-2). A launch's names the index does not list are followed (plan 215): a former alias opens its
 * live graph.
 */
export async function listForLaunch(
  launches: Launches,
  gen: number,
  named: string[] | null,
  fresh: boolean,
  deps: { list: (named: string[] | null) => Promise<Listed>; alive: () => boolean },
  apply: (result: ListResult) => void,
): Promise<void> {
  let result: ListResult;
  try {
    const { graphs, renamed = [], refused = [] } = await deps.list(named);
    const names = named === null ? null : followedNames(named, renamed);
    result = { kind: "graphs", graphs, pick: firstGraphs(graphs, names), named: names, fresh, renamed, refused };
  } catch (error) {
    result = { kind: "failed", error };
  }
  if (deps.alive() && launches.latest(gen)) apply(result);
}

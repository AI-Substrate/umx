import { describe, expect, it, vi } from "vitest";
import { firstGraphs, type LaunchDeps, Launches, listForLaunch, requestedGraphs, takeLaunch, watchLaunch } from "@/lib/launch";
import type { GraphSummary } from "@/lib/types";

const row = (alias: string, more: Partial<GraphSummary> = {}): GraphSummary => ({
  alias, id: "1", nodes: 1, edges: 0, secured: false, state: "loaded", folder: "present", openable: true, reason: null, fix: null, ...more,
});
const graphs = [
  row("vault", { secured: true }),
  row("demo"),
  row("later", { state: "idle", nodes: null, edges: null }),
  row("gone", { state: "unloaded", openable: false, reason: "It was unloaded.", fix: "umx graph load /h/gone --as gone" }),
];
const CODE = "A".repeat(43);
/** A launch code the server refuses (used, stale or made up). */
const BAD = "Z".repeat(43);

describe("the launch link", () => {
  it("reads the named graphs from the fragment, beside the launch code", () => {
    expect(requestedGraphs(`#t=${CODE}&g=demo`)).toEqual(["demo"]);
    // umx ui --graph a,b,c writes the commas as %2C (ws009 M1b1).
    expect(requestedGraphs(`#t=${CODE}&g=alpha%2Cbeta%2Cgamma`)).toEqual(["alpha", "beta", "gamma"]);
    expect(requestedGraphs("#g=my%20graph&t=x")).toEqual(["my graph"]);
    expect(requestedGraphs(`#t=${CODE}`)).toBeNull();
    expect(requestedGraphs("#g=%E0%A4%A")).toBeNull();
    // Several graphs on one canvas (plan 133): a comma list, each once, at most MAX_OPEN.
    expect(requestedGraphs(`#t=${CODE}&g=a,b,,a,c`)).toEqual(["a", "b", "c"]);
    expect(requestedGraphs(`#g=${Array.from({ length: 20 }, (_v, i) => `g${i}`).join(",")}`)).toHaveLength(12);
    expect(requestedGraphs("#g=,")).toBeNull();
  });

  const pick = (aliases: string[], named: boolean, more: object = {}) => ({ aliases, named, missing: [], blocked: [], ...more });

  it("opens a secured or idle graph only when it is named, and never one the picker would not open", () => {
    expect(firstGraphs(graphs, null)).toEqual(pick(["demo"], false));
    expect(firstGraphs(graphs, ["vault"])).toEqual(pick(["vault"], true));
    expect(firstGraphs(graphs, ["later"])).toEqual(pick(["later"], true));
    expect(firstGraphs(graphs, ["nope"])).toEqual(pick(["demo"], false, { missing: ["nope"] }));
    expect(firstGraphs(graphs, ["gone"])).toEqual(pick(["demo"], false, { blocked: [graphs[3]] }));
    // An idle graph is never opened by itself: opening it would load it.
    expect(firstGraphs([row("later", { state: "idle" })], null)).toEqual(pick([], false));
  });

  it("opens every named graph it can, secured ones included, and says which it could not (plan 133)", () => {
    expect(firstGraphs(graphs, ["demo", "vault", "nope", "gone", "later"])).toEqual(
      pick(["demo", "vault", "later"], true, { missing: ["nope"], blocked: [graphs[3]] }),
    );
  });
});

/** A tab: its address, history and the launch POSTs it made. */
function tab(hash: string) {
  const location = { hash, pathname: "/", search: "" };
  const posted: string[] = [];
  const deps: LaunchDeps = {
    location,
    history: { replaceState: (_d, _u, url) => (location.hash = new URL(url, "http://h").hash) },
    post: async (code) => {
      posted.push(code);
      return code !== BAD;
    },
  };
  return { location, posted, deps };
}

describe("taking a launch code", () => {
  it("hands the code over at load and clears the fragment", async () => {
    const t = tab(`#t=${CODE}&g=demo`);
    expect(await takeLaunch(t.deps, new Launches())).toEqual({ named: ["demo"], gen: 1 });
    expect(t.posted).toEqual([CODE]);
    expect(t.location.hash).toBe("");
  });

  it("does nothing for a fragment that is not a launch link, but still clears it", async () => {
    const t = tab("#nothing");
    expect(await takeLaunch(t.deps, new Launches())).toBeNull();
    expect(t.posted).toEqual([]);
    expect(t.location.hash).toBe("");
  });

  it("takes a new link that lands in an open tab (hashchange)", async () => {
    const t = tab("");
    const win = new EventTarget();
    const launched = vi.fn();
    const stop = watchLaunch(win, t.deps, new Launches(), launched);
    const second = "B".repeat(43);
    t.location.hash = `#t=${second}&g=later`;
    win.dispatchEvent(new Event("hashchange"));
    await vi.waitFor(() => expect(launched).toHaveBeenCalledWith({ named: ["later"], gen: 1 }));
    expect(t.posted).toEqual([second]);
    expect(t.location.hash).toBe("");
    // Clearing the fragment fires no hashchange, and after stop() a new link is not read.
    stop();
    t.location.hash = `#t=${CODE}`;
    win.dispatchEvent(new Event("hashchange"));
    await new Promise((r) => setTimeout(r, 10));
    expect(t.posted).toEqual([second]);
    expect(launched).toHaveBeenCalledTimes(1);
  });

  it("opens and loads nothing for a lone g=, plain or secured, at load or on hashchange (security N1)", async () => {
    for (const hash of ["#g=demo", "#g=vault", "#g=later&x=1"]) {
      const t = tab(hash);
      expect(await takeLaunch(t.deps, new Launches())).toBeNull();
      expect(t.posted).toEqual([]);
      expect(t.location.hash).toBe("");
      const win = new EventTarget();
      const launched = vi.fn();
      watchLaunch(win, t.deps, new Launches(), launched);
      t.location.hash = hash;
      win.dispatchEvent(new Event("hashchange"));
      await new Promise((r) => setTimeout(r, 10));
      expect(launched).not.toHaveBeenCalled();
      expect(t.location.hash).toBe("");
    }
  });

  it("names no graph when the server refuses the code", async () => {
    const t = tab(`#t=${BAD}&g=vault`);
    expect(await takeLaunch(t.deps, new Launches())).toEqual({ named: null, gen: 1 });
    expect(t.posted).toEqual([BAD]);
    expect(t.location.hash).toBe("");
  });

  it("still reports the launch, naming no graph, when the POST fails", async () => {
    const t = tab(`#t=${CODE}&g=vault`);
    t.deps.post = async () => {
      throw new Error("offline");
    };
    expect(await takeLaunch(t.deps, new Launches())).toEqual({ named: null, gen: 1 });
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const FAILED = new Error("offline");

/**
 * A tab as graph-app wires it: each launch's POST and graph-list read are held until the test settles them, and the
 * view state is what the results applied.
 */
function liveTab() {
  const location = { hash: "", pathname: "/", search: "" };
  const posts = new Map<string, ReturnType<typeof deferred<boolean>>>();
  const lists = new Map<string | null, ReturnType<typeof deferred<GraphSummary[]>>>();
  const keyOf = (named: string[] | null) => named?.join(",") ?? null;
  const deps: LaunchDeps = {
    location,
    history: { replaceState: (_d, _u, url) => (location.hash = new URL(url, "http://h").hash) },
    post: (code) => {
      const d = deferred<boolean>();
      posts.set(code, d);
      return d.promise;
    },
  };
  const launches = new Launches();
  const view = { alias: null as string | null, failed: false, applied: 0, alive: true };
  const refresh = (named: string[] | null, fresh: boolean, gen: number) => {
    const d = deferred<GraphSummary[]>();
    lists.set(keyOf(named), d);
    return listForLaunch(
      launches,
      gen,
      named,
      fresh,
      { list: () => d.promise.then((graphs) => ({ graphs })), alive: () => view.alive },
      (r) => {
        view.applied++;
        if (r.kind === "failed") view.failed = true;
        else {
          view.failed = false;
          if (r.fresh && r.pick.named) view.alias = r.pick.aliases.join(",");
          else view.alias ??= r.pick.aliases.join(",") || null;
        }
      },
    );
  };
  const win = new EventTarget();
  const delivered: (string | null)[] = [];
  const stop = watchLaunch(win, deps, launches, (l) => {
    delivered.push(keyOf(l.named));
    void refresh(l.named, true, l.gen);
  });
  const land = (code: string, graph: string) => {
    location.hash = `#t=${code}&g=${graph}`;
    win.dispatchEvent(new Event("hashchange"));
  };
  return { posts, lists, launches, view, delivered, stop, land, win, location, refresh };
}

const A = "A".repeat(43);
const B = "B".repeat(43);
const list = [row("older"), row("newer")];

describe("overlapping launches (review R222-2)", () => {
  it("control: one link exchanges, lists and opens its graph", async () => {
    const t = liveTab();
    t.land(A, "newer");
    expect(t.location.hash).toBe("");
    t.posts.get(A)!.resolve(true);
    await flush();
    t.lists.get("newer")!.resolve(list);
    await flush();
    expect(t.view.alias).toBe("newer");
  });

  it("keeps the newer link when the older exchange settles last", async () => {
    const t = liveTab();
    t.land(A, "older");
    t.land(B, "newer");
    t.posts.get(B)!.resolve(true);
    await flush();
    t.posts.get(A)!.resolve(true);
    await flush();
    expect(t.delivered).toEqual(["newer"]);
    t.lists.get("newer")!.resolve(list);
    await flush();
    expect(t.view.alias).toBe("newer");
    expect(t.lists.has("older")).toBe(false);
  });

  it("keeps the newer link when the older graph-list read settles last", async () => {
    const t = liveTab();
    t.land(A, "older");
    t.posts.get(A)!.resolve(true);
    await flush();
    t.land(B, "newer");
    t.posts.get(B)!.resolve(true);
    await flush();
    expect(t.delivered).toEqual(["older", "newer"]);
    t.lists.get("newer")!.resolve(list);
    await flush();
    t.lists.get("older")!.resolve(list);
    await flush();
    expect(t.view.alias).toBe("newer");
    expect(t.view.applied).toBe(1);
  });

  it("never fails a view a newer link opened with an obsolete failure", async () => {
    const t = liveTab();
    t.land(A, "older");
    t.posts.get(A)!.resolve(false);
    await flush();
    t.land(B, "newer");
    t.posts.get(B)!.resolve(true);
    await flush();
    t.lists.get("newer")!.resolve(list);
    await flush();
    expect(t.view.failed).toBe(false);
    t.lists.get(null)!.reject(FAILED);
    await flush();
    expect(t.view.failed).toBe(false);
    expect(t.view.alias).toBe("newer");
  });

  it("changes nothing for an exchange or a read that settles after unsubscribe", async () => {
    const t = liveTab();
    t.land(A, "older");
    t.stop();
    t.posts.get(A)!.resolve(true);
    await flush();
    expect(t.delivered).toEqual([]);
    const pending = t.refresh(["older"], true, t.launches.now());
    t.view.alive = false;
    t.lists.get("older")!.resolve(list);
    await pending;
    expect(t.view.applied).toBe(0);
  });

  it("a lone g= landing mid-launch neither launches nor cancels the launch in flight", async () => {
    const t = liveTab();
    t.land(A, "newer");
    t.location.hash = "#g=vault";
    t.win.dispatchEvent(new Event("hashchange"));
    t.posts.get(A)!.resolve(true);
    await flush();
    expect(t.delivered).toEqual(["newer"]);
    expect(t.posts.size).toBe(1);
  });

  it("a background re-list started before a launch changes nothing", async () => {
    const t = liveTab();
    const background = t.refresh(null, false, t.launches.now());
    t.land(B, "newer");
    t.posts.get(B)!.resolve(true);
    await flush();
    t.lists.get("newer")!.resolve(list);
    await flush();
    t.lists.get(null)!.reject(FAILED);
    await background;
    expect(t.view).toMatchObject({ alias: "newer", failed: false, applied: 1 });
  });
});

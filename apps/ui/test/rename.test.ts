// Plan 215: the view follows a graph rename (ws006 plan 209, #395). The graph-addressing expert's rules: the live
// alias comes from umx (`umx resolve umx:<old>`), never from a hint's text or a walk; one probe per alias per session,
// only for an alias the index does not list; the mapping lives in memory; identity is the graph id; only a graph in
// the open set or the launch link switches; umx's refusals switch nothing.
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, onRenamed, renamedOf } from "@/lib/api";
import { followedNames, listForLaunch, Launches } from "@/lib/launch";
import { composeRenames, idsWithPending, movedSelection, renameKey, renameNotice, renamesLanded, renameTarget } from "@/lib/rename";
import { hintItems } from "@/components/hit-list";
import { Publisher } from "@/lib/selection";
import { ShadowCache } from "@/lib/shadows-read";
import { followNamed, graphIds, listGraphs, loadNode, loadSnapshot, Probes } from "@/lib/snapshot";
import { hintsOf, UmxError, warningsOf } from "@/lib/umx";

const GA = "01JC8XA4AAAAAAAAAAAAAAAAAA";
const GB = "01JC8XA4BBBBBBBBBBBBBBBBBB";
const N1 = "01JC8XA4N0000000000000000A";

type Row = { alias: string; id: string; state?: string; policy?: string };
type Resolved = { status: string; redirected_from?: string; graph: string; graph_id: string } | UmxError;

/** A fake umx: the index rows, what `resolve umx:<alias>` answers per alias, and every verb and graph asked. */
function fakeUmx(rows: Row[], resolves: Record<string, Resolved>) {
  const asked: [string, string | undefined][] = [];
  const env = (verb: string, data: unknown, hints: unknown[] = [], warnings: unknown[] = []) => ({ umx: 79, command: verb, status: "ok", data, warnings, errors: [], hints });
  const read = (async (verb: string, args: { graph?: string } = {}) => {
    asked.push([verb, args.graph]);
    if (verb === "graph list") {
      return env(verb, { rows: rows.map((r) => ({ folder: "present", state: "loaded", policy: "open", nodes: 3, edges: 1, ...r })), total: rows.length, truncated: false });
    }
    if (verb === "resolve graph") {
      const r = resolves[args.graph!];
      if (r === undefined) throw new UmxError("graph_not_loaded", `graph ${args.graph} is not loaded`, "umx graph list");
      if (r instanceof UmxError) throw r;
      return env(verb, { ...r, kind: "node", address: `umx:${r.graph}` });
    }
    if (verb === "ontology show") return env(verb, { symmetric: [] });
    if (verb === "node list") {
      // A redirect reported both ways (review R404-3: a warning too), beside an unrelated warning that stays.
      const redirect = { code: "alias_redirected", message: "gone is a former alias of graph live; umx used live. Name it live from now on.", next: "umx graph guide live" };
      return env(verb, { rows: [{ id: N1, type: "note", title: "n", status: "open", evidence: [`umx:gone/${N1}`] }], total: 1, truncated: false }, [redirect], [
        redirect,
        { code: "evidence_unchecked", message: "some evidence was not checked", next: "umx check" },
      ]);
    }
    if (verb === "edge list") {
      // An edge end in another graph named under a former alias: shown as stored, never followed.
      return env(verb, { rows: [{ id: N1, type: "cites", from: N1, to: N1, from_graph: GA, to_graph: GB, graph_id: GA }], total: 1, truncated: false });
    }
    if (verb === "node get") {
      return env(verb, { node: { id: N1, type: "note", title: "n", status: "open", evidence: [{ source: `umx:gone/${N1}` }] } });
    }
    if (verb === "neighbours") return env(verb, { rows: [] });
    return env(verb, {});
  }) as unknown as Parameters<typeof graphIds>[0];
  const resolvesSent = () => asked.filter(([v]) => v === "resolve graph").map(([, g]) => g);
  return { read, asked, resolvesSent };
}

async function refusal(p: Promise<unknown>): Promise<UmxError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(UmxError);
    return e as UmxError;
  }
  throw new Error("expected a refusal");
}

describe("the server finds a rename (lib/snapshot graphIds)", () => {
  it("refuses a renamed open graph's former alias with its live alias and graph id, from umx resolve, once per alias", async () => {
    const u = fakeUmx([{ alias: "gamma", id: GA }], { alpha: { status: "redirected", redirected_from: "alpha", graph: "gamma", graph_id: GA } });
    const probes = new Probes();
    const e = await refusal(graphIds(u.read, "alpha", probes));
    expect(e.code).toBe("ui_graph_renamed");
    expect(e.httpStatus).toBe(409);
    expect(e.toJSON()).toEqual({ code: "ui_graph_renamed", message: "alpha was renamed to gamma.", next: null, renamed: { from: "alpha", to: "gamma", id: GA } });
    // Every route asks again (events, rows, publish, the snapshot): umx is asked once.
    for (let i = 0; i < 3; i++) await refusal(graphIds(u.read, "alpha", probes));
    expect(u.resolvesSent()).toEqual(["alpha"]);
    // A listed alias is never probed.
    await graphIds(u.read, "gamma", probes);
    expect(u.resolvesSent()).toEqual(["alpha"]);
  });

  it("shares one probe between concurrent reads; a read that failed on the view's side is asked again only after the index changes", async () => {
    const rows: Row[] = [{ alias: "gamma", id: GA }];
    const u = fakeUmx(rows, { alpha: { status: "redirected", redirected_from: "alpha", graph: "gamma", graph_id: GA } });
    const probes = new Probes();
    await Promise.all([1, 2, 3].map(() => refusal(graphIds(u.read, "alpha", probes))));
    expect(probes.sent).toBe(1);
    const flaky = new Probes();
    let fail = true;
    const read = (async (verb: string, args: { graph?: string }) => {
      if (verb === "resolve graph" && fail) {
        fail = false;
        throw new UmxError("ui_umx_output", "umx did not answer with JSON.");
      }
      return (u.read as unknown as (v: string, a: unknown) => Promise<unknown>)(verb, args);
    }) as unknown as Parameters<typeof graphIds>[0];
    expect((await refusal(graphIds(read, "alpha", flaky))).code).toBe("ui_graph_not_listed");
    expect((await refusal(graphIds(read, "alpha", flaky))).code).toBe("ui_graph_not_listed");
    expect(flaky.sent).toBe(1);
    rows.push({ alias: "beta", id: GB });
    expect((await refusal(graphIds(read, "alpha", flaky))).code).toBe("ui_graph_renamed");
    expect(flaky.sent).toBe(2);
  });

  it("ends a chain a -> b -> c on c as umx resolves it, and follows a later rename by graph id with no second probe", async () => {
    const rows: Row[] = [{ alias: "c", id: GA }];
    const u = fakeUmx(rows, { a: { status: "redirected", redirected_from: "a", graph: "c", graph_id: GA } });
    const probes = new Probes();
    expect((await refusal(graphIds(u.read, "a", probes))).renamed).toEqual({ from: "a", to: "c", id: GA });
    rows[0] = { alias: "d", id: GA };
    expect((await refusal(graphIds(u.read, "a", probes))).renamed).toEqual({ from: "a", to: "d", id: GA });
    expect(u.resolvesSent()).toEqual(["a"]);
    expect(u.asked.filter(([v]) => v !== "graph list" && v !== "resolve graph")).toEqual([]);
  });

  it("passes umx's former_alias_ambiguous and graph_not_loaded on as refusals that name no live alias, once per alias", async () => {
    const ambiguous = new UmxError("former_alias_ambiguous", "2 loaded graphs once had the alias shared; name a live alias.", "umx graph list");
    const u = fakeUmx([{ alias: "one", id: GA }, { alias: "two", id: GB }], { shared: ambiguous });
    const probes = new Probes();
    for (const alias of ["shared", "shared", "unknown", "unknown"]) {
      const e = await refusal(graphIds(u.read, alias, probes));
      expect(e.code).toBe(alias === "shared" ? "former_alias_ambiguous" : "graph_not_loaded");
      expect(e.httpStatus).toBe(409);
      expect(e.renamed).toBeNull();
      expect(JSON.stringify(e.toJSON())).not.toContain("renamed");
    }
    expect(u.resolvesSent()).toEqual(["shared", "unknown"]);
  });

  it("switches to nothing umx did not resolve: a plain ok, another alias's redirect, an unlisted or unopenable live graph", async () => {
    const u = fakeUmx([{ alias: "gamma", id: GA }, { alias: "gone_on_purpose", id: GB, state: "unloaded" }], {
      ok_one: { status: "ok", graph: "ok_one", graph_id: GA },
      other: { status: "redirected", redirected_from: "someone_else", graph: "gamma", graph_id: GA },
      stale: { status: "redirected", redirected_from: "stale", graph: "vanished", graph_id: "01JC8XA4CCCCCCCCCCCCCCCCCC" },
      parked: { status: "redirected", redirected_from: "parked", graph: "gone_on_purpose", graph_id: GB },
    });
    for (const alias of ["ok_one", "other", "stale", "parked"]) {
      expect((await refusal(graphIds(u.read, alias, new Probes()))).code).toBe("ui_graph_not_listed");
    }
  });

  it("a secured graph renamed: the refusal names only the alias asked and the live alias the index already shows", async () => {
    const u = fakeUmx([{ alias: "vault_v2", id: GA, policy: "secured" }], { vault: { status: "redirected", redirected_from: "vault", graph: "vault_v2", graph_id: GA } });
    const listed = await listGraphs(u.read);
    expect(listed[0]).toMatchObject({ alias: "vault_v2", secured: true, nodes: null, edges: null });
    const e = await refusal(graphIds(u.read, "vault", new Probes()));
    expect(e.toJSON()).toEqual({ code: "ui_graph_renamed", message: "vault was renamed to vault_v2.", next: null, renamed: { from: "vault", to: "vault_v2", id: GA } });
  });

  it("drops shadows answers whose open set names the former alias, and keeps the others (no rekeying)", async () => {
    const cache = new ShadowCache();
    const got = { rows: [], groups: [], skipped: [], compared: null } as unknown as Parameters<ShadowCache["put"]>[1];
    cache.put(JSON.stringify(["d", ["alpha", "beta"], "", ["alpha=1"]]), got);
    cache.put(JSON.stringify(["d", ["beta"], "", ["beta=1"]]), got);
    cache.forget("alpha");
    expect(cache.size).toBe(1);
    expect(cache.get(JSON.stringify(["d", ["beta"], "", ["beta=1"]]))).toBe(got);
  });

  it("never probes an alias met in evidence, an edge end or a hint about another graph (rule 4)", async () => {
    const u = fakeUmx([{ alias: "alpha", id: GA }, { alias: "live", id: GB }], { gone: { status: "redirected", redirected_from: "gone", graph: "live", graph_id: GB } });
    const snap = await loadSnapshot(u.read, "alpha");
    await loadNode(u.read, "alpha", N1);
    expect(u.resolvesSent()).toEqual([]);
    // The hint about `gone` is dropped from the strip, never acted on.
    expect(snap.hints).toEqual([]);
  });

  it("drops alias_redirected from warnings as well as hints, keeping every other warning (review R404-3)", async () => {
    const u = fakeUmx([{ alias: "alpha", id: GA }], {});
    const snap = await loadSnapshot(u.read, "alpha");
    expect(snap.warnings?.map((w) => w.code)).toEqual(["evidence_unchecked"]);
    // What the hint strip shows (graph-app: hintItems over the snapshot's warnings and hints).
    expect(hintItems(snap.warnings ?? [], snap.hints ?? []).map((h) => h.code)).toEqual(["evidence_unchecked"]);
    expect(warningsOf({ warnings: [{ code: "alias_redirected", message: "m", next: null }] })).toEqual([]);
  });

  it("never asks again about an alias, however many others it asked about since (review R404-2)", async () => {
    const u = fakeUmx([{ alias: "gamma", id: GA }], {});
    const graphs = await listGraphs(u.read);
    const probes = new Probes();
    await probes.ask(u.read, "first", graphs);
    for (let i = 0; i < 300; i++) await probes.ask(u.read, `other_${i}`, graphs);
    await probes.ask(u.read, "first", graphs);
    expect(probes.sent).toBe(301);
    expect(u.resolvesSent().filter((a) => a === "first")).toEqual(["first"]);
  });

  it("past its cap asks about no new alias and refuses it as not listed, and still never asks again about the old ones", async () => {
    const u = fakeUmx([{ alias: "gamma", id: GA }], { a: { status: "redirected", redirected_from: "a", graph: "gamma", graph_id: GA } });
    const probes = new Probes(2);
    expect((await refusal(graphIds(u.read, "a", probes))).code).toBe("ui_graph_renamed");
    expect((await refusal(graphIds(u.read, "b", probes))).code).toBe("graph_not_loaded");
    expect((await refusal(graphIds(u.read, "c", probes))).code).toBe("ui_graph_not_listed");
    expect((await refusal(graphIds(u.read, "a", probes))).code).toBe("ui_graph_renamed");
    expect((await refusal(graphIds(u.read, "b", probes))).code).toBe("graph_not_loaded");
    expect(u.resolvesSent()).toEqual(["a", "b"]);
  });

  it("asks again about a refused alias once the graph index changes, once, and never before", async () => {
    const rows: Row[] = [{ alias: "gamma", id: GA }];
    const resolves: Record<string, Resolved> = {};
    const u = fakeUmx(rows, resolves);
    const probes = new Probes();
    expect((await refusal(graphIds(u.read, "later", probes))).code).toBe("graph_not_loaded");
    expect((await refusal(graphIds(u.read, "later", probes))).code).toBe("graph_not_loaded");
    expect(u.resolvesSent()).toEqual(["later"]);
    // The graph is renamed from `later` and the index changes: asked once more, then kept.
    resolves.later = { status: "redirected", redirected_from: "later", graph: "delta", graph_id: GB };
    rows.push({ alias: "delta", id: GB });
    expect((await refusal(graphIds(u.read, "later", probes))).renamed).toEqual({ from: "later", to: "delta", id: GB });
    rows.push({ alias: "epsilon", id: "01JC8XA4CCCCCCCCCCCCCCCCCC" });
    expect((await refusal(graphIds(u.read, "later", probes))).renamed).toEqual({ from: "later", to: "delta", id: GB });
    expect(u.resolvesSent()).toEqual(["later", "later"]);
  });
});

describe("a launch link with a former alias (lib/snapshot followNamed, lib/launch)", () => {
  it("asks umx only about the link's unlisted names, and opens the live graph", async () => {
    const u = fakeUmx([{ alias: "gamma", id: GA }, { alias: "beta", id: GB }], {
      alpha: { status: "redirected", redirected_from: "alpha", graph: "gamma", graph_id: GA },
      shared: new UmxError("former_alias_ambiguous", "2 loaded graphs once had the alias shared.", "umx graph list"),
    });
    const graphs = await listGraphs(u.read);
    const probes = new Probes();
    const got = await followNamed(u.read, graphs, ["alpha", "beta", "shared", "--x", "alpha"], probes);
    expect(got.renamed).toEqual([{ from: "alpha", to: "gamma", id: GA }]);
    expect(got.refused).toEqual([{ alias: "shared", issue: { code: "former_alias_ambiguous", message: "2 loaded graphs once had the alias shared.", next: "umx graph list" } }]);
    expect(u.resolvesSent()).toEqual(["alpha", "shared"]);
    await followNamed(u.read, graphs, ["alpha"], probes);
    expect(u.resolvesSent()).toEqual(["alpha", "shared"]);
  });

  it("opens the live graph the list followed, once, in the link's order", async () => {
    expect(followedNames(["alpha", "beta", "gamma"], [{ from: "alpha", to: "gamma", id: GA }])).toEqual(["gamma", "beta"]);
    const launches = new Launches();
    const gen = launches.begin();
    let result: unknown = null;
    const graphs = [
      { alias: "gamma", id: GA, nodes: 1, edges: 0, secured: false, state: "loaded", folder: "present", openable: true, reason: null, fix: null },
    ];
    await listForLaunch(
      launches,
      gen,
      ["alpha"],
      true,
      { list: async (named) => ({ graphs, renamed: named?.includes("alpha") ? [{ from: "alpha", to: "gamma", id: GA }] : [] }), alive: () => true },
      (r) => (result = r),
    );
    expect(result).toMatchObject({ kind: "graphs", named: ["gamma"], pick: { aliases: ["gamma"], named: true, missing: [] }, renamed: [{ from: "alpha", to: "gamma" }] });
  });

  it("plant control: without following, the same link opens nothing it names", async () => {
    const launches = new Launches();
    let result: unknown = null;
    const graphs = [
      { alias: "gamma", id: GA, nodes: 1, edges: 0, secured: false, state: "loaded", folder: "present", openable: true, reason: null, fix: null },
    ];
    await listForLaunch(launches, launches.begin(), ["alpha"], true, { list: async () => ({ graphs }), alive: () => true }, (r) => (result = r));
    expect(result).toMatchObject({ pick: { named: false, missing: ["alpha"] } });
  });
});

describe("the browser switches (lib/rename)", () => {
  const ids = new Map([
    ["alpha", GA],
    ["beta", GB],
  ]);

  it("puts the live alias in the former one's place, so the graph keeps its colour rank", () => {
    expect(renameTarget(["beta", "alpha"], ids, { from: "alpha", to: "gamma", id: GA })).toEqual(["beta", "gamma"]);
  });

  it("makes one tab when the live alias is open too, as --graph a,c does (rule 3)", () => {
    expect(renameTarget(["alpha", "beta", "gamma"], new Map([...ids, ["gamma", GA]]), { from: "alpha", to: "gamma", id: GA })).toEqual(["beta", "gamma"]);
  });

  it("switches only a graph in the open set (rule 6), and only the same graph by id (rule 3)", () => {
    expect(renameTarget(["beta"], ids, { from: "alpha", to: "gamma", id: GA })).toBeNull();
    expect(renameTarget(["alpha"], ids, { from: "alpha", to: "gamma", id: GB })).toBeNull();
  });

  // As GraphApp's followRename and the loading effect's land run them: each rename composes with those still pending,
  // the set lands only once every live graph has arrived, and the selection moves through every pending rename.
  function follow(open: string[], ids: Map<string, string>, pending: Map<string, string>, r: { from: string; to: string; id: string }) {
    const target = renameTarget(open, idsWithPending(ids, pending), r);
    return target === null ? null : { target, renames: composeRenames(pending, r) };
  }

  it("keeps both graphs' selected nodes when a second open graph is renamed before the first replacement lands (review R404-1)", () => {
    const selection = { graph: "alpha,beta", ids: [`alpha/${N1}`, `beta/${N1}`], primary: `alpha/${N1}` };
    const first = follow(["alpha", "beta"], ids, new Map(), { from: "alpha", to: "gamma", id: GA })!;
    // gamma's read is still pending when beta is renamed.
    const second = follow(first.target, ids, first.renames, { from: "beta", to: "delta", id: GB })!;
    expect(second.target).toEqual(["gamma", "delta"]);
    expect([...second.renames]).toEqual([
      ["alpha", "gamma"],
      ["beta", "delta"],
    ]);
    // gamma arrives first: nothing lands yet, so beta's nodes stay on screen and selected.
    expect(renamesLanded(second.renames, second.target, ["alpha", "beta"], ["gamma"])).toBe(false);
    expect(renamesLanded(second.renames, second.target, ["alpha", "beta"], ["gamma", "delta"])).toBe(true);
    const canvas = new Set([`gamma/${N1}`, `delta/${N1}`]);
    expect(movedSelection(selection, "gamma,delta", second.renames, (id) => canvas.has(id))).toEqual({
      graph: "gamma,delta",
      ids: [`gamma/${N1}`, `delta/${N1}`],
      primary: `gamma/${N1}`,
    });
    // Control: the second rename alone (round 1) loses the first graph's selection.
    expect(movedSelection(selection, "gamma,delta", new Map([["beta", "delta"]]), (id) => canvas.has(id)).primary).toBeNull();
  });

  it("follows a pending graph renamed again before it lands, to the last live alias", () => {
    const first = follow(["alpha", "beta"], ids, new Map(), { from: "alpha", to: "gamma", id: GA })!;
    const again = follow(first.target, ids, first.renames, { from: "gamma", to: "omega", id: GA })!;
    expect(again.target).toEqual(["omega", "beta"]);
    expect(renameKey(`alpha/${N1}`, again.renames)).toBe(`omega/${N1}`);
    // A pending live alias stands for its graph's id only: another graph's id there still switches nothing.
    expect(follow(first.target, ids, first.renames, { from: "gamma", to: "omega", id: GB })).toBeNull();
  });

  it("moves canvas keys of the renamed graph only, and says it once", () => {
    const renames = new Map([["alpha", "gamma"]]);
    expect(renameKey(`alpha/${N1}`, renames)).toBe(`gamma/${N1}`);
    expect(renameKey(`beta/${N1}`, renames)).toBe(`beta/${N1}`);
    expect(renameNotice({ from: "alpha", to: "gamma" })).toEqual({ code: "ui_graph_renamed", message: "alpha was renamed to gamma.", next: null });
  });

  it("never shows alias_redirected as a hint card, and keeps every other hint", () => {
    const hints = hintsOf({
      hints: [
        { code: "alias_redirected", message: "alpha is a former alias of graph gamma; umx used gamma. Name it gamma from now on.", next: "umx graph guide gamma" },
        { code: "graph_loaded_on_demand", message: "loaded", next: null },
      ],
    });
    expect(hints.map((h) => h.code)).toEqual(["graph_loaded_on_demand"]);
  });
});

describe("the browser hears of a rename from any read (lib/api)", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("tells its listeners each well-formed ui_graph_renamed, and nothing else; the mapping is never stored", async () => {
    const heard: unknown[] = [];
    const stop = onRenamed((r) => heard.push(r));
    const answer = (error: unknown) => new Response(JSON.stringify({ error }), { status: 409 });
    const replies = [
      answer({ code: "ui_graph_renamed", message: "alpha was renamed to gamma.", next: null, renamed: { from: "alpha", to: "gamma", id: GA } }),
      answer({ code: "ui_graph_renamed", message: "x", next: null, renamed: { from: "alpha", to: "--all", id: GA } }),
      answer({ code: "former_alias_ambiguous", message: "x", next: null }),
    ];
    const storage = { setItem: vi.fn() };
    vi.stubGlobal("localStorage", storage);
    vi.stubGlobal("fetch", async () => replies.shift());
    for (let i = 0; i < 3; i++) await api("/api/graphs/alpha/snapshot").catch(() => null);
    stop();
    expect(heard).toEqual([{ from: "alpha", to: "gamma", id: GA }]);
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(renamedOf({ code: "ui_graph_renamed", renamed: { from: "a", to: "a", id: GA } })).toBeNull();
  });
});

describe("the published selection follows (lib/selection Publisher)", () => {
  function harness(failGraph: string | null = null) {
    const posts: [string, string[]][] = [];
    const timers = new Map<number, () => void>();
    let n = 0;
    const pub = new Publisher({
      post: async (graph, nodes) => {
        posts.push([graph, [...nodes]]);
        if (graph === failGraph) throw Object.assign(new Error("renamed"), { code: "ui_graph_renamed" });
      },
      changed: () => {},
      setTimer: (fn) => {
        timers.set(++n, fn);
        return n;
      },
      clearTimer: (t) => timers.delete(t as number),
      asIssue: (e) => ({ code: (e as { code?: string }).code ?? "x", message: String(e) }),
    });
    const tick = async () => {
      for (const [k, fn] of [...timers]) {
        timers.delete(k);
        fn();
      }
      // The publisher's drain runs on microtasks only: let each queued post settle.
      for (let i = 0; i < 20; i++) await Promise.resolve();
    };
    return { pub, posts, timers, tick };
  }

  it("publishes under the live alias and never through the former one, even while the screen still names it", async () => {
    const h = harness();
    h.pub.update(new Map([["alpha", [N1]]]));
    await h.tick();
    expect(h.posts).toEqual([["alpha", [N1]]]);
    h.pub.rename("alpha", "gamma");
    h.pub.update(new Map([["alpha", [N1]]]));
    await h.tick();
    h.pub.update(new Map([["gamma", [N1]]]));
    await h.tick();
    expect(h.posts).toEqual([
      ["alpha", [N1]],
      ["gamma", [N1]],
    ]);
  });

  it("does not retry or report a publish refused as renamed", async () => {
    const h = harness("alpha");
    h.pub.update(new Map([["alpha", [N1]]]));
    await h.tick();
    await h.tick();
    expect(h.posts).toEqual([["alpha", [N1]]]);
    expect(h.timers.size).toBe(0);
  });
});

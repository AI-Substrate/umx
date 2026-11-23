// The picker lists this machine's graph index (`umx graph list --all`, req-116) and opens only what it may: loaded
// and idle graphs in a present folder (a named read loads an idle one on demand, F72). Rows are shaped like the
// daemon's serializer (crates/umx-daemon/src/verbs.rs graph_index).
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { GraphList, Scopes } from "@/components/graph-picker";
import { allGraphs, draftToggled, groupOf, matchGraphs, scopeGraphs, selectMatching, toggled } from "@/lib/graph-set";
import { MAX_OPEN } from "@/lib/graph/keys";
import { firstGraphs } from "@/lib/launch";
import { searchGraph, similarTo } from "@/lib/search";
import { graphIds, listGraphs, listScopes, loadNode, loadSnapshot } from "@/lib/snapshot";
import { createUmx, UMX_PROTOCOL, UmxError, type Exec } from "@/lib/umx";

const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA";
const N = "01NODEAAAAAAAAAAAAAAAAAAAA";
const index = (alias: string, id: string, state: string, more: object = {}) => ({
  alias, id, path: `/h/g/${alias}`, state, folder: "present", last_loaded: null, nodes: null, edges: null, model: null,
  policy: "open", repo: null, reload: `umx graph load /h/g/${alias} --as ${alias}`, last_used: null, unloads_in_s: null, ...more,
});
const ROWS = [
  index("live", GA, "loaded", { nodes: 3, edges: 2, last_loaded: "2026-11-12T00:00:00Z" }),
  index("later", "01GRAPHBBBBBBBBBBBBBBBBBBB", "idle", { nodes: 40, edges: 12 }),
  index("vault", "01GRAPHCCCCCCCCCCCCCCCCCCC", "idle", { policy: "secured" }),
  index("named", "01GRAPHDDDDDDDDDDDDDDDDDDD", "loaded", { policy: "named+trusted", nodes: 1, edges: 0 }),
  index("gone", "01GRAPHEEEEEEEEEEEEEEEEEEE", "unloaded", { nodes: 5, edges: 1 }),
  index("broke", "01GRAPHFFFFFFFFFFFFFFFFFFF", "not_reloaded"),
  index("moved", "01GRAPHGGGGGGGGGGGGGGGGGGG", "idle", { folder: "missing" }),
  index("away", "01GRAPHHHHHHHHHHHHHHHHHHHH", "idle", { folder: "outside_roots", reload: "umx graph list --all" }),
];

const member = (id: string, alias: string, secured: boolean | null) => ({ id, alias, folder: `/h/g/${alias}`, loaded: true, secured });
const SCOPES = [
  // live and later open; vault and named are secured (counted, never named); gone cannot open; bad names are skipped.
  { name: "work", members: [member(GA, "live", false), member("01GRAPHBBBBBBBBBBBBBBBBBBB", "later", false), member("01GRAPHCCCCCCCCCCCCCCCCCCC", "vault", null), member("01GRAPHDDDDDDDDDDDDDDDDDDD", "named", true), member("01GRAPHEEEEEEEEEEEEEEEEEEE", "gone", false)] },
  { name: "Bad Name", members: [member(GA, "live", false)] },
];

const env = (command: string, data: unknown, warnings: unknown[] = []) =>
  JSON.stringify({ umx: UMX_PROTOCOL, command, status: warnings.length ? "warning" : "ok", data, warnings, errors: [] });
const ON_DEMAND = { code: "graph_loaded_on_demand", message: "later was not loaded; loaded it from /h/g/later.", next: null };

function harness(rows: unknown[] = ROWS) {
  const exec = vi.fn<Exec>(async (_bin, argv) => {
    const [a, b] = argv;
    const verb = `${a} ${b}`;
    if (verb === "graph list") return { stdout: env(verb, { rows }), failed: false, detail: "" };
    if (verb === "node list") {
      const rowsN = [{ id: N, type: "problem", title: "t", status: "open" }];
      return { stdout: env(verb, { rows: rowsN, total: 1, truncated: false }, argv.includes("later") ? [ON_DEMAND] : []), failed: false, detail: "" };
    }
    if (verb === "edge list") return { stdout: env(verb, { rows: [], total: 0, truncated: false }), failed: false, detail: "" };
    if (verb === "ontology show") return { stdout: env(verb, { symmetric: ["conflicts_with"] }), failed: false, detail: "" };
    if (verb === "scope list") return { stdout: env(verb, { rows: SCOPES, total: SCOPES.length, truncated: false }), failed: false, detail: "" };
    return { stdout: env(argv[0] === "search" ? "search" : verb, { rows: [] }), failed: false, detail: "" };
  });
  return { exec, read: createUmx({ exec, bin: () => "/u", env: () => ({}) }) };
}

async function refused(p: Promise<unknown>): Promise<UmxError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(UmxError);
    return e as UmxError;
  }
  throw new Error("expected a refusal");
}

describe("the picker lists the graph index", () => {
  it("reads graph list --all and marks each row openable or not, with why and the fix", async () => {
    const { exec, read } = harness();
    const list = await listGraphs(read);
    expect(exec.mock.calls[0][1]).toEqual(["graph", "list", "--all"]);
    const by = Object.fromEntries(list.map((g) => [g.alias, g]));
    expect(list.filter((g) => g.openable).map((g) => g.alias)).toEqual(["live", "later", "vault", "named"]);
    expect(by.gone).toMatchObject({ openable: false, fix: "umx graph load /h/g/gone --as gone" });
    expect(by.gone.reason).toContain("unloaded");
    expect(by.broke).toMatchObject({ openable: false, fix: "umx graph load /h/g/broke --as broke" });
    expect(by.broke.reason).toContain("did not come back");
    expect(by.moved).toMatchObject({ openable: false, fix: "umx graph load <new folder> --as moved" });
    expect(by.away).toMatchObject({ openable: false, fix: "umx config set roots <folders>" });
    // Every named-visibility policy is locked; counts umx withholds stay null, never 0.
    expect(list.filter((g) => g.secured).map((g) => g.alias)).toEqual(["vault", "named"]);
    expect(by.vault.nodes).toBeNull();
    expect(by.later.nodes).toBe(40);
  });

  it("dims what it cannot open, says why and how to fix it, and labels counts from a last load", async () => {
    const list = await listGraphs(harness().read);
    const html = renderToStaticMarkup(<GraphList graphs={list} open={["live"]} onPick={() => {}} onToggle={() => {}} />);
    const items = html.split("<li ").slice(1);
    const item = (alias: string) => items.find((i) => i.includes(`>${alias}</span>`)) ?? "";
    for (const alias of ["live", "later", "vault", "named"]) {
      expect(item(alias)).toContain('data-openable="true"');
      expect(item(alias)).toContain("<button");
    }
    for (const alias of ["gone", "broke", "moved", "away"]) {
      expect(item(alias)).toContain('data-openable="false"');
      expect(item(alias)).not.toContain("<button");
      expect(item(alias)).toContain("opacity-60");
    }
    expect(item("gone")).toContain("umx graph load /h/g/gone --as gone");
    expect(item("away")).toContain("outside roots");
    expect(item("live")).toContain("3 · 2");
    expect(item("live")).not.toContain("last load");
    expect(item("later")).toContain("40 · 12 (last load)");
    expect(item("vault")).toContain('aria-label="secured"');
    expect(item("vault")).not.toContain(" · ");
  });

  it("never shows a named graph's totals, even loaded with counts from umx (review R222-1)", async () => {
    const rows = [
      index("sec", GA, "loaded", { policy: "secured", nodes: 11, edges: 7 }),
      index("nany", "01GRAPHBBBBBBBBBBBBBBBBBBB", "loaded", { policy: "named+any", nodes: 12, edges: 8 }),
      index("ntrust", "01GRAPHCCCCCCCCCCCCCCCCCCC", "loaded", { policy: "named+trusted", nodes: 13, edges: 9 }),
      index("odd", "01GRAPHDDDDDDDDDDDDDDDDDDD", "loaded", { policy: "someday+new", nodes: 14, edges: 10 }),
      index("open", "01GRAPHEEEEEEEEEEEEEEEEEEE", "loaded", { policy: "open", nodes: 15, edges: 11 }),
      index("trusted", "01GRAPHFFFFFFFFFFFFFFFFFFF", "loaded", { policy: "open+trusted", nodes: 16, edges: 12 }),
      index("nap", "01GRAPHGGGGGGGGGGGGGGGGGGG", "idle", { policy: "open", nodes: 17, edges: 13 }),
    ];
    const list = await listGraphs(harness(rows).read);
    const html = renderToStaticMarkup(<GraphList graphs={list} open={[]} onPick={() => {}} />);
    const item = (alias: string) => html.split("<li ").find((i) => i.includes(`>${alias}</span>`)) ?? "";
    // The counts element, not bare digits: the lock icon's SVG path holds digits of its own.
    for (const alias of ["sec", "nany", "ntrust", "odd"]) {
      expect(list.find((g) => g.alias === alias)).toMatchObject({ secured: true, openable: true, nodes: null, edges: null });
      expect(item(alias)).toContain('aria-label="secured"');
      expect(item(alias)).toContain("<button");
      expect(item(alias)).not.toContain("tabular-nums");
      expect(item(alias)).not.toContain(" · ");
    }
    expect(item("open")).toContain("tabular-nums");
    expect(item("open")).toContain("15 · 11");
    expect(item("open")).not.toContain("last load");
    expect(item("trusted")).toContain("16 · 12");
    expect(item("nap")).toContain("17 · 13 (last load)");
    // Nothing opens one by itself: the first graph is the loaded open one.
    expect(firstGraphs(list, null).aliases).toEqual(["open"]);
  });

  it("never runs a named read of a graph the picker would not open", async () => {
    for (const alias of ["gone", "broke", "moved", "away"]) {
      const { exec, read } = harness();
      expect((await refused(loadSnapshot(read, alias))).code).toBe("ui_graph_not_open");
      expect((await refused(loadNode(read, alias, N))).code).toBe("ui_graph_not_open");
      expect((await refused(searchGraph(read, alias, "hybrid", "tank"))).code).toBe("ui_graph_not_open");
      expect((await refused(searchGraph(read, alias, "title", "tank"))).code).toBe("ui_graph_not_open");
      expect((await refused(similarTo(read, alias, N))).code).toBe("ui_graph_not_open");
      expect(exec.mock.calls.every((c) => c[1][0] === "graph")).toBe(true);
    }
    const { read } = harness();
    expect((await refused(loadSnapshot(read, "nope"))).code).toBe("ui_graph_not_listed");
  });

  it("refuses an alias remembered in two folders with none loaded, rather than pick one", async () => {
    const { exec, read } = harness([index("twin", GA, "idle"), index("twin", GA, "idle", { path: "/h/other" })]);
    expect((await refused(loadSnapshot(read, "twin"))).code).toBe("ui_graph_ambiguous");
    // Only the index is read (once more, fresh, before refusing): never a named read that would load one.
    expect(exec.mock.calls.map((c) => c[1].slice(0, 2).join(" "))).toEqual(["graph list", "graph list"]);
  });

  it("shares one index read between checks, keeps it briefly, and reads it fresh before any refusal", async () => {
    const { exec, read } = harness();
    await Promise.all([loadSnapshot(read, "live"), loadSnapshot(read, "named")]);
    await loadSnapshot(read, "live");
    expect(exec.mock.calls.filter((c) => c[1][0] === "graph").length).toBe(1);
    await refused(loadSnapshot(read, "nowhere"));
    expect(exec.mock.calls.filter((c) => c[1][0] === "graph").length).toBe(2);
  });

  it("opens an idle graph with a named read and passes umx's graph_loaded_on_demand warning on", async () => {
    const { exec, read } = harness();
    const snap = await loadSnapshot(read, "later");
    expect(exec.mock.calls.map((c) => c[1].slice(0, 2).join(" "))).toEqual(["graph list", "node list", "ontology show", "edge list"]);
    expect(snap.warnings?.map((w) => w.code)).toEqual(["graph_loaded_on_demand"]);
    expect(snap.nodes).toHaveLength(1);
    expect((await loadSnapshot(read, "live")).warnings).toEqual([]);
  });

  it("names only loaded graphs as loaded when telling rows apart", async () => {
    const { aliasOf } = await graphIds(harness().read, "later");
    expect([...aliasOf.values()].sort()).toEqual(["later", "live", "named"]);
  });
});

describe("several graphs in the picker (plan 133, D69 (7))", () => {
  it("a click or the box names a graph: adds it, or takes it off down to one, never past MAX_OPEN", () => {
    expect(toggled(["a"], "b")).toEqual(["a", "b"]);
    expect(toggled(["a", "b"], "a")).toEqual(["b"]);
    expect(toggled(["a"], "a")).toEqual(["a"]);
    const twelve = Array.from({ length: 12 }, (_v, i) => `g${i}`);
    expect(toggled(twelve, "x")).toEqual(twelve);
  });

  it("opens only the loaded row of two that share an alias; the other is dimmed with its folder's command", async () => {
    const twin = [
      index("dup_alias", "01GRAPHAAAAAAAAAAAAAAAAAAA", "loaded", { policy: "secured" }),
      index("dup_alias", "01GRAPHBBBBBBBBBBBBBBBBBBB", "idle", { folder: "missing", path: "/h/old/dup", reload: "umx graph load /h/old/dup --as dup_alias" }),
      index("dup", "01GRAPHCCCCCCCCCCCCCCCCCCC", "loaded"),
      index("dup", "01GRAPHDDDDDDDDDDDDDDDDDDD", "idle", { path: "/h/g/dup2", reload: "umx graph load /h/g/dup2 --as dup" }),
      index("pair", "01GRAPHEEEEEEEEEEEEEEEEEEE", "idle", { reload: "umx graph load /h/g/p1 --as pair" }),
      index("pair", "01GRAPHFFFFFFFFFFFFFFFFFFF", "idle", { reload: "umx graph load /h/g/p2 --as pair" }),
    ];
    const list = await listGraphs(harness(twin).read);
    expect(list.map((g) => [g.alias, g.id.slice(-1), g.openable])).toEqual([
      ["dup_alias", "A", true],
      ["dup_alias", "B", false],
      ["dup", "C", true],
      ["dup", "D", false],
      ["pair", "E", false],
      ["pair", "F", false],
    ]);
    expect(list[3]).toMatchObject({ fix: "umx graph load /h/g/dup2 --as <new alias>" });
    expect(list[3].reason).toContain("Another folder's graph is loaded as dup");
    expect(list[4].reason).toContain("in 2 folders");
    expect(list[1].reason).toContain("folder holds no graph");
    // The open alias marks only the row that opens; each row keeps its own identity.
    const html = renderToStaticMarkup(<GraphList graphs={list} open={["dup_alias", "dup"]} onPick={() => {}} onToggle={() => {}} />);
    const rows = html.split("<li ").slice(1);
    expect(rows.map((r) => r.includes('data-open="true"'))).toEqual([true, false, true, false, false, false]);
    expect(rows.map((r) => r.includes("<button"))).toEqual([true, false, true, false, false, false]);
  });

  it("checks the draft's graphs, offers 'only' beside each, lets the last one go, and stops at the limit", async () => {
    const list = await listGraphs(harness().read);
    const html = renderToStaticMarkup(<GraphList graphs={list} open={["live", "named"]} onPick={() => {}} onToggle={() => {}} />);
    const item = (alias: string) => html.split("<li ").find((i) => i.includes(`>${alias}</span>`)) ?? "";
    expect(item("live")).toContain('data-open="true"');
    expect(item("named")).toContain('aria-checked="true"');
    expect(item("later")).toContain('aria-checked="false"');
    expect(item("later")).toContain('data-testid="graph-only"');
    expect(item("gone")).not.toContain('data-testid="graph-toggle"');
    const one = renderToStaticMarkup(<GraphList graphs={list} open={["live"]} onPick={() => {}} onToggle={() => {}} />);
    expect(one.split("<li ").find((i) => i.includes(">live</span>"))).not.toContain("aria-disabled");
    expect(draftToggled(["live"], "live")).toEqual([]);
    const full = Array.from({ length: MAX_OPEN }, (_, i) => `g${i}`);
    expect(draftToggled(full, "live")).toEqual(full);
    const capped = renderToStaticMarkup(<GraphList graphs={list} open={full} onPick={() => {}} onToggle={() => {}} />);
    expect(capped.split("<li ").find((i) => i.includes(">later</span>"))).toContain('aria-disabled="true"');
  });

  it("filters by every word of the name and groups rows by what opening them does", async () => {
    const list = await listGraphs(harness().read);
    expect(matchGraphs(list, "").length).toBe(list.length);
    expect(matchGraphs(list, "  LA ").map((g) => g.alias)).toEqual(["later"]);
    expect(matchGraphs(list, "a e").map((g) => g.alias)).toEqual(["later", "named"]);
    expect(matchGraphs(list, "zzz")).toEqual([]);
    expect(Object.fromEntries(list.map((g) => [g.alias, groupOf(g)]))).toMatchObject({ live: "loaded", named: "loaded", later: "idle", vault: "idle", gone: "blocked", moved: "blocked" });
    const html = renderToStaticMarkup(<GraphList graphs={list} open={[]} onPick={() => {}} onToggle={() => {}} query="at" />);
    expect(html).toContain("l<mark");
    expect(html).toContain(">at</mark>er");
  });

  it("'Select matching' adds the loaded matches to the draft as 'Open all' would, counting what it left out", async () => {
    const list = await listGraphs(harness().read);
    // later is idle (selecting it would load it); named is secured.
    expect(selectMatching(["later"], list, false)).toEqual({ aliases: ["later", "live"], leftOut: { secured: 2, unavailable: 0 } });
    expect(selectMatching([], matchGraphs(list, "named"), true)).toEqual({ aliases: ["named"], leftOut: { secured: 0, unavailable: 0 } });
    const full = Array.from({ length: MAX_OPEN }, (_, i) => `g${i}`);
    expect(selectMatching(full, list, true)).toEqual({ aliases: full, leftOut: { secured: 0, unavailable: 2 } });
  });

  it("'Open all' opens loaded open graphs and counts the secured ones it left out, unless the person includes them", async () => {
    const list = await listGraphs(harness().read);
    // later is idle (opening it would load it); vault and named are secured.
    expect(allGraphs(list)).toEqual({ aliases: ["live"], leftOut: { secured: 2, unavailable: 0 } });
    expect(allGraphs(list, true)).toEqual({ aliases: ["live", "named"], leftOut: { secured: 0, unavailable: 0 } });
  });

  it("a scope opens its open members; secured ones are counted, never shown by name, and open only with the toggle on", async () => {
    const { exec, read } = harness();
    const scopes = await listScopes(read);
    expect(exec.mock.calls.map((c) => c[1].join(" "))).toEqual(["graph list --all", "scope list"]);
    expect(scopes).toEqual([{ name: "work", graphs: ["live", "later"], securedGraphs: ["vault", "named"], secured: 2, unavailable: 1 }]);
    expect(JSON.stringify(scopes)).not.toContain("/h/g");
    expect(scopeGraphs(scopes[0], false)).toEqual({ aliases: ["live", "later"], leftOut: { secured: 2, unavailable: 1 } });
    expect(scopeGraphs(scopes[0], true)).toEqual({ aliases: ["live", "later", "vault", "named"], leftOut: { secured: 0, unavailable: 1 } });
    const html = renderToStaticMarkup(<Scopes scopes={scopes} onPick={() => {}} includeSecured={false} />);
    expect(html).toContain("2 graphs");
    expect(html).toContain("+3 left out");
    expect(html).not.toContain("vault");
    expect(html).not.toContain("named");
  });
});

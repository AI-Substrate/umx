// Search, title find and similar_to against replies recorded from real umx (test/fixtures/search.json, written by
// plan 104's search-fixture.sh (not published) on the synthetic graph searchfix). The view must
// run exactly the recorded argv and list the same nodes in umx's order, with umx's warnings as given.
import { describe, expect, it, vi } from "vitest";
import { searchGraph, similarTo } from "@/lib/search";
import { createUmx, QUERY_MAX, UmxError, type Exec } from "@/lib/umx";
import fixture from "./fixtures/search.json";

type Env = { status: string; data: { rows: { id: string; graph?: string }[] } | null; warnings: { code: string; message: string }[] };
// The recorder keys each reply by its argv as Python's json.dumps writes it; key them as JSON.stringify does.
const calls: Record<string, Env> = Object.fromEntries(
  Object.entries(fixture.calls as unknown as Record<string, Env>).map(([k, v]) => [JSON.stringify(JSON.parse(k)), v]),
);

// The recording is plain `graph list`; the view reads `graph list --all` (plan 113), whose loaded rows add state,
// folder and policy (crates/umx-daemon/src/verbs.rs graph_index).
const graphIndex = {
  ...fixture.graph_list,
  data: {
    ...fixture.graph_list.data,
    rows: fixture.graph_list.data.rows.map((r) => ({ ...r, state: "loaded", folder: "present", policy: r.effective_policy })),
  },
};

/** The argv of every read but the graph index, which each search reads first to check the graph may be opened, and the
 * flagged node reads that vouch for each hit's title (ws009 M2b). */
const reads = (exec: { mock: { calls: [string, string[], ...unknown[]][] } }) =>
  exec.mock.calls.map((c) => c[1]).filter((argv) => argv[0] !== "graph" && !(argv[0] === "node" && argv[1] === "get"));

/** `node get <id> --columns ...` as umx answers it: the row, with `flags` as its secret_suspected. */
const nodeRow = (argv: string[], flags: string[] = []) => ({
  umx: 29,
  command: "node get",
  status: "ok",
  data: { node: { id: argv[2], type: "item", title: `title of ${argv[2]}`, status: "open", ...(flags.length > 0 ? { secret_suspected: flags } : {}) } },
  warnings: [],
  errors: [],
});

/** An exec that answers the graph index as recorded and every other read with `reply`. */
const indexed = (reply: (argv: string[]) => unknown) =>
  vi.fn<Exec>(async (_bin, argv) => ({ stdout: JSON.stringify(argv[0] === "graph" ? graphIndex : reply(argv)), failed: false, detail: "" }));

function recorded() {
  const exec = vi.fn<Exec>(async (_bin, argv) => {
    const key = JSON.stringify(argv);
    const env =
      argv[0] === "graph" ? graphIndex : argv[0] === "node" && argv[1] === "get" ? nodeRow(argv) : key === JSON.stringify(fixture.early.argv) && early ? fixture.early.env : calls[key];
    if (!env) throw new Error(`no recording for ${key}`);
    return { stdout: JSON.stringify(env), failed: false, detail: "" };
  });
  let early = false;
  const read = createUmx({ exec, bin: () => "/opt/umx/bin/umx", env: () => ({ UMX_HOME: "/h", UMX_OUTPUT: "json" }) });
  return { exec, read, useEarly: () => (early = true) };
}

const ids = (argv: string[]) => calls[JSON.stringify(argv)].data!.rows.map((r) => r.id);
const G = fixture.graph;

describe("results equal what umx returned", () => {
  for (const q of fixture.queries.find) {
    it(`title find "${q}" = umx node find`, async () => {
      const { exec, read } = recorded();
      const argv = ["node", "find", q, "--graph", G, "--limit", "20"];
      const got = await searchGraph(read, G, "title", q);
      expect(reads(exec)).toEqual([argv]);
      expect(got.hits.map((h) => h.id)).toEqual(ids(argv));
      expect(got.hits.length).toBeGreaterThan(0);
    });
  }
  for (const q of fixture.queries.search) {
    it(`search "${q}" = umx search`, async () => {
      const { exec, read } = recorded();
      const argv = ["search", q, "--graph", G, "--limit", "20"];
      const got = await searchGraph(read, G, "hybrid", q);
      expect(reads(exec)).toEqual([argv]);
      expect(got.hits.map((h) => h.id)).toEqual(ids(argv));
      expect(got.warnings.map((w) => w.code)).toEqual(calls[JSON.stringify(argv)].warnings.map((w) => w.code));
    });
  }
  for (const [title, id] of Object.entries(fixture.similar)) {
    it(`similar to "${title}" = umx neighbours --relation similar_to`, async () => {
      const { exec, read } = recorded();
      const argv = ["neighbours", id, "--graph", G, "--relation", "similar_to", "--direction", "out", "--limit", "100"];
      const got = await similarTo(read, G, id);
      expect(reads(exec)[0]).toEqual(argv);
      expect(got.hits.map((h) => h.id)).toEqual(ids(argv));
      expect(got.note).toContain("text-embedding-3-large@256");
    });
  }

  it("keeps a search that umx answered with status warning, and shows the warning as given", async () => {
    const { read, useEarly } = recorded();
    useEarly();
    const env = fixture.early.env as unknown as Env;
    expect(env.status).toBe("warning");
    const got = await searchGraph(read, G, "hybrid", "water tank");
    expect(got.hits.map((h) => h.id)).toEqual(env.data!.rows.map((r) => r.id));
    expect(got.warnings).toEqual(env.warnings.map((w) => expect.objectContaining({ code: w.code, message: w.message })));
    expect(got.warnings.map((w) => w.code)).toContain("similarity_unavailable");
  });

  it("finds through typos and meaning, as umx did", () => {
    const titles = (argv: string[]) => (calls[JSON.stringify(argv)].data!.rows as unknown as { title: string }[]).map((r) => r.title);
    expect(titles(["node", "find", "watr pump", "--graph", G, "--limit", "20"])).toContain("Water pump for the rain tank");
    expect(titles(["search", "keeping plants from drying out", "--graph", G, "--limit", "20"])).toContain("Spread mulch to keep soil moist");
  });
});

describe("search text never becomes a flag", () => {
  async function refused(p: Promise<unknown>): Promise<string> {
    try {
      await p;
    } catch (e) {
      expect(e).toBeInstanceOf(UmxError);
      return (e as UmxError).code;
    }
    throw new Error("expected a refusal");
  }

  it("passes -x as one plain word right after the verb, which umx reads as text (recorded: -oven)", async () => {
    const { exec, read } = recorded();
    await searchGraph(read, G, "title", "-oven");
    await searchGraph(read, G, "hybrid", "-oven");
    expect(reads(exec).map((a) => a.slice(0, 3))).toEqual([["node", "find", "-oven"], ["search", "-oven", "--graph"]]);
    // umx found the oven both ways: it took -oven as words, not as a flag.
    expect(ids(["node", "find", "-oven", "--graph", G, "--limit", "20"]).length).toBeGreaterThan(0);
  });

  it("refuses text starting with -- (umx has no -- terminator) before anything runs", async () => {
    const { exec, read } = recorded();
    for (const q of ["--graph other", "--graph=other", "  --limit 9", "--scope=both x", "---x"]) {
      expect(await refused(searchGraph(read, G, "hybrid", q))).toBe("ui_bad_query");
      expect(await refused(searchGraph(read, G, "title", q))).toBe("ui_bad_query");
    }
    expect(exec).not.toHaveBeenCalled();
  });

  it("keeps --graph inside the text as part of one argv element", async () => {
    const exec = indexed(() => ({ ...fixture.early.env, status: "ok", warnings: [] }));
    const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
    await searchGraph(read, G, "hybrid", "tank --graph other");
    expect(reads(exec)[0]).toEqual(["search", "tank --graph other", "--graph", G, "--limit", "20"]);
  });

  it("refuses -h and -V, which umx reads as its help or version flag, and keeps other -x words (R199-1)", async () => {
    const { exec, read } = recorded();
    for (const q of ["-h", " -h ", "-V"]) {
      expect(await refused(searchGraph(read, G, "title", q))).toBe("ui_bad_query");
      expect(await refused(searchGraph(read, G, "hybrid", q))).toBe("ui_bad_query");
    }
    expect(exec).not.toHaveBeenCalled();
    const any = indexed((argv) => ({ umx: 29, command: argv[0] === "node" ? "node find" : "search", status: "ok", data: { rows: [] }, warnings: [], errors: [] }));
    const read2 = createUmx({ exec: any, bin: () => "/u", env: () => ({}) });
    for (const q of ["-x", "-hello", "-H", "-h tank", "help"]) {
      await searchGraph(read2, G, "title", q);
      await searchGraph(read2, G, "hybrid", q);
    }
    expect(reads(any).map((a) => a[a[0] === "node" ? 2 : 1])).toEqual(["-x", "-x", "-hello", "-hello", "-H", "-H", "-h tank", "-h tank", "help", "help"]);
  });

  it("refuses a reply for another command (umx's help) instead of showing no matches (R199-1)", async () => {
    // What umx answers to `umx search -h --graph searchfix` (recorded 2026-11-11, protocol 29, markdown cut).
    const help = { umx: 29, command: "help", status: "ok", data: { markdown: "# Search\n..." }, warnings: [], errors: [] };
    const exec = indexed(() => help);
    const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
    expect(await refused(searchGraph(read, G, "hybrid", "tank"))).toBe("ui_umx_output");
    expect(await refused(searchGraph(read, G, "title", "tank"))).toBe("ui_umx_output");
    expect(await refused(similarTo(read, G, "01ZZZZZZZZZZZZZZZZZZZZZZZZ"))).toBe("ui_umx_output");
    // Rows alone are not enough: they must answer the verb that ran.
    const other = { ...help, command: "node list", data: { rows: [{ id: "01ZZZZZZZZZZZZZZZZZZZZZZZZ", graph: G }] } };
    const exec2 = indexed(() => other);
    const read2 = createUmx({ exec: exec2, bin: () => "/u", env: () => ({}) });
    expect(await refused(searchGraph(read2, G, "hybrid", "tank"))).toBe("ui_umx_output");
    expect(await refused(similarTo(read2, G, "01ZZZZZZZZZZZZZZZZZZZZZZZZ"))).toBe("ui_umx_output");
  });

  it("refuses a 10 kB string, control characters and text with no word", async () => {
    const { exec, read } = recorded();
    expect(await refused(searchGraph(read, G, "hybrid", "a".repeat(10_240)))).toBe("ui_bad_query");
    expect(await refused(searchGraph(read, G, "title", "a".repeat(QUERY_MAX + 1)))).toBe("ui_bad_query");
    for (const q of ["tank\nother", "tank\u0000", "tank\u001b[2J", "a\u2028b", "tab\there", "\u0085x", "  ", "-", "?!"]) {
      expect(await refused(searchGraph(read, G, "hybrid", q))).toBe("ui_bad_query");
    }
    expect(await refused(searchGraph(read, "--graph", "hybrid", "tank"))).toBe("ui_bad_graph");
    expect(await refused(similarTo(read, G, "--graph"))).toBe("ui_bad_id");
    expect(exec).not.toHaveBeenCalled();
  });

  it("takes the text at QUERY_MAX and collapses spaces as a shell would", async () => {
    const exec = indexed(() => ({ ...fixture.early.env, status: "ok", warnings: [] }));
    const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
    await searchGraph(read, G, "hybrid", "  water    tank ");
    await searchGraph(read, G, "hybrid", "b".repeat(QUERY_MAX));
    expect(reads(exec).map((a) => a[1])).toEqual(["water tank", "b".repeat(QUERY_MAX)]);
  });
});

describe("hit titles come only from a read that flags them (ws009 M2b)", () => {
  const A = "01AAAAAAAAAAAAAAAAAAAAAAAA";
  const B = "01BBBBBBBBBBBBBBBBBBBBBBBB";
  const C = "01CCCCCCCCCCCCCCCCCCCCCCCC";
  const find = {
    umx: 29,
    command: "node find",
    status: "ok",
    data: { rows: [A, B, C].map((id) => ({ id, type: "item", title: `raw ${id}`, status: "open", score: 1 })) },
    warnings: [],
    errors: [],
  };
  it("shows a flagged title as type and short id, and one its read cannot vouch for the same way", async () => {
    const exec = indexed((argv) => {
      if (argv[0] === "node" && argv[1] === "get") {
        if (argv[2] === C) return { umx: 29, command: "node get", status: "error", data: null, warnings: [], errors: [{ code: "graph_busy", message: "busy" }] };
        return nodeRow(argv, argv[2] === B ? ["field:title"] : []);
      }
      return find;
    });
    const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
    const got = await searchGraph(read, G, "title", "raw");
    expect(got.hits.map((h) => h.title)).toEqual([`title of ${A}`, "item · BBBBBB", "item · CCCCCC"]);
    expect(JSON.stringify(got)).not.toContain("raw ");
    const asked = exec.mock.calls.map((c) => c[1]).filter((a) => a[0] === "node" && a[1] === "get");
    expect(asked.map((a) => a.slice(2))).toEqual([A, B, C].map((id) => [id, "--graph", G, "--columns", "id,type,title,status,updated,entity_key"]));
  });
});

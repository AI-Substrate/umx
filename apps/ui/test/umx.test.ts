import { describe, expect, it, vi } from "vitest";
import { createUmx, EDGES_BY_ID, FLAGS_FROM_ONE, NODE_COLUMNS, parseEnvelope, READ_VERB_NAMES, resolveBin, umxEnv, UMX_PROTOCOL, UMX_PROTOCOL_MIN, UmxError, type Exec } from "@/lib/umx";

const ok = (data: unknown, umx = UMX_PROTOCOL) =>
  JSON.stringify({ umx, command: "x", status: "ok", data, warnings: [], errors: [] });
const ID = "01ZZZZZZZZZZZZZZZZZZZZZZZZ";

function harness(stdout = ok({ rows: [], total: 0, truncated: false })) {
  const exec = vi.fn<Exec>(async () => ({ stdout, failed: false, detail: "" }));
  const read = createUmx({ exec, bin: () => "/opt/umx/bin/umx", env: () => ({ UMX_HOME: "/h", UMX_OUTPUT: "json" }) });
  return { exec, read };
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

describe("umx adapter", () => {
  it("allows only the eighteen read verbs", () => {
    expect(READ_VERB_NAMES.sort()).toEqual([
      "daemon status", "edge list", "edges by id", "embeddings status", "entity show", "entity show loaded", "graph list", "neighbours",
      "node find", "node get", "node list", "node row", "ontology show", "resolve graph", "scope list", "search", "similar to", "similarity pairs",
    ]);
  });

  it("resolves a graph address only, the alias checked (plan 215)", async () => {
    const { exec, read } = harness();
    await read("resolve graph", { graph: "demo", id: ID });
    expect(exec.mock.calls[0][1]).toEqual(["resolve", "umx:demo"]);
    await expect(read("resolve graph", { graph: "--all" })).rejects.toMatchObject({ code: "ui_bad_graph" });
    expect(exec).toHaveBeenCalledTimes(1);
  });

  it("reads shadows over exactly the open graphs, a lone one named twice, at umx's own floor, all rows in one call, as lean rows (ws009 M1c, s34a, plan 184)", async () => {
    const { exec, read } = harness();
    await read("similarity pairs", { graph: "demo", with: ["beta"], offset: 10000 });
    expect(exec.mock.calls[0][1]).toEqual(["similarity", "pairs", "--graph", "demo,beta", "--same-graph", "--k", "10", "--limit", "250000", "--offset", "10000", "--rows", "ids"]);
    await read("similarity pairs", { graph: "demo", model: "text-embedding-3-small@256" });
    expect(exec.mock.calls[1][1]).toEqual(["similarity", "pairs", "--graph", "demo,demo", "--same-graph", "--k", "10", "--limit", "250000", "--offset", "0", "--rows", "ids", "--model", "text-embedding-3-small@256"]);
    await read("embeddings status", { graph: "demo" });
    expect(exec.mock.calls[2][1]).toEqual(["embeddings", "status"]);
    await expect(read("similarity pairs", { graph: "demo", model: "--all" })).rejects.toMatchObject({ code: "ui_bad_model" });
    expect(exec.mock.calls.flat(2).join(" ")).not.toContain("--min");
  });

  it("reads a node's entity by address over the checked open graphs, and over the loaded ones by id (ws009 M2)", async () => {
    const { exec, read } = harness();
    await read("entity show", { graph: "demo", id: ID, with: ["beta", "demo"] });
    expect(exec.mock.calls[0][1]).toEqual(["entity", "show", `umx:demo/${ID}`, "--graph", "demo,beta"]);
    await read("entity show loaded", { graph: "demo", id: ID, with: ["beta"] });
    expect(exec.mock.calls[1][1]).toEqual(["entity", "show", ID, "--graph", "demo"]);
    // One open graph is named twice: umx reads a list with a comma as exactly those graphs (review B2).
    await read("entity show", { graph: "demo", id: ID, with: ["demo"] });
    expect(exec.mock.calls[2][1]).toEqual(["entity", "show", `umx:demo/${ID}`, "--graph", "demo,demo"]);
    await expect(read("entity show", { graph: "demo", id: ID, with: ["--scope"] })).rejects.toMatchObject({ code: "ui_bad_graph" });
    await expect(read("entity show", { graph: "demo", id: "01X" })).rejects.toMatchObject({ code: "ui_bad_id" });
    expect(exec).toHaveBeenCalledTimes(3);
  });

  it("reads the saved scopes with fixed argv, and edges by id across the open graphs as one checked --graph list (plan 133)", async () => {
    const { exec, read } = harness();
    await read("scope list", { graph: "--evil", query: "--x" });
    expect(exec.mock.calls[0][1]).toEqual(["scope", "list"]);
    await read("edges by id", { graph: "demo", ids: [ID], with: ["beta", "demo", "gamma"] });
    expect(exec.mock.calls[1][1]).toEqual(["query", EDGES_BY_ID, "--graph", "demo,beta,gamma", "--param-json", `ids=["${ID}"]`, "--param-json", 'graph="demo"', "--limit", "500"]);
    for (const bad of [["--scope"], ["a,b"], ["Beta"], Array.from({ length: 12 }, (_v, i) => `g${i}`)]) {
      await expect(read("edges by id", { graph: "demo", ids: [ID], with: bad })).rejects.toMatchObject({ code: "ui_bad_graph" });
    }
    expect(exec).toHaveBeenCalledTimes(2);
    await read("ontology show", { graph: "demo", query: "--x" });
    expect(exec.mock.calls.at(-1)![1]).toEqual(["ontology", "show", "--graph", "demo"]);
  });

  it("reads edges by id with fixed query text and the ids in one JSON parameter (plan 127), and a node row with its flags", async () => {
    const { exec, read } = harness();
    await read("edges by id", { graph: "demo", ids: [ID] });
    await read("node row", { graph: "demo", id: ID });
    expect(exec.mock.calls[0][1]).toEqual(["query", EDGES_BY_ID, "--graph", "demo", "--param-json", `ids=["${ID}"]`, "--param-json", 'graph="demo"', "--limit", "500"]);
    // Only the graph's own edges, filtered inside the query before its limit (review B2).
    expect(EDGES_BY_ID).toContain("r.graph = $graph");
    // The ids are a parameter: the text never changes with them.
    expect(EDGES_BY_ID).toContain("$ids");
    // A node is read by `node get --columns`, whose reply flags what it shows (ws009 M2b).
    expect(exec.mock.calls[1][1]).toEqual(["node", "get", ID, "--graph", "demo", "--columns", NODE_COLUMNS]);
  });

  it("refuses an id list that is empty, over 500, or holds anything but full ids, before execFile", async () => {
    const { exec, read } = harness();
    for (const verb of ["edges by id"]) {
      for (const ids of [[], Array.from({ length: 501 }, () => ID), ["--graph"], [`${ID}"]`], [ID.slice(0, 10)], undefined]) {
        const code = (await refused(read(verb, { graph: "demo", ids: ids as string[] | undefined }))).code;
        expect(["ui_bad_ids", "ui_bad_id"]).toContain(code);
      }
      expect((await refused(read(verb, { graph: "--all", ids: [ID] }))).code).toBe("ui_bad_graph");
    }
    await read("edges by id", { graph: "demo", ids: Array.from({ length: 500 }, () => ID) });
    expect(exec).toHaveBeenCalledTimes(1);
  });

  it("refuses a write verb before execFile", async () => {
    const { exec, read } = harness();
    for (const verb of ["node add", "node edit", "edge add", "batch", "graph unload", "query", "__proto__", "toString"]) {
      expect((await refused(read(verb, { graph: "demo" }))).code).toBe("ui_verb_refused");
    }
    expect(exec).not.toHaveBeenCalled();
  });

  it("refuses a bad id before execFile", async () => {
    const { exec, read } = harness();
    for (const id of ["--graph", "01zzzzzzzzzzzzzzzzzzzzzzzz", `${ID}X`, "01ZZZZZZZZ", `${ID.slice(0, 25)};`, ""]) {
      expect((await refused(read("node get", { graph: "demo", id }))).code).toBe("ui_bad_id");
      expect((await refused(read("neighbours", { graph: "demo", id }))).code).toBe("ui_bad_id");
    }
    expect(exec).not.toHaveBeenCalled();
  });

  it("refuses a bad alias or offset before execFile", async () => {
    const { exec, read } = harness();
    for (const graph of ["--scope", "-x", "../demo", "Demo", "a b", ""]) {
      expect((await refused(read("node list", { graph }))).code).toBe("ui_bad_graph");
    }
    expect((await refused(read("node list", { graph: "demo", offset: -1 }))).code).toBe("ui_bad_offset");
    expect((await refused(read("node list", { graph: "demo", offset: 1.5 }))).code).toBe("ui_bad_offset");
    expect(exec).not.toHaveBeenCalled();
  });

  it("runs fixed argument arrays with the fixed env", async () => {
    const { exec, read } = harness(ok({ node: {} }));
    await read("node get", { graph: "demo_d1", id: ID });
    await read("node list", { graph: "demo", offset: 20000 });
    expect(exec.mock.calls[0]).toEqual([
      "/opt/umx/bin/umx",
      ["node", "get", ID, "--graph", "demo_d1"],
      { UMX_HOME: "/h", UMX_OUTPUT: "json" },
    ]);
    expect(exec.mock.calls[1][1]).toEqual([
      "node", "list", "--graph", "demo", "--limit", "10000", "--offset", "20000", "--columns", "id,type,title,status,updated,entity_key",
    ]);
  });

  it("refuses a protocol older than it reads, with a clear message", () => {
    const e = (() => {
      try {
        parseEnvelope(ok({}, UMX_PROTOCOL_MIN - 1), false, "");
      } catch (err) {
        return err as UmxError;
      }
    })();
    expect(e?.code).toBe("ui_protocol_mismatch");
    expect(e?.message).toContain(`protocol ${UMX_PROTOCOL_MIN} or newer`);
  });

  it("reads the oldest and the tested protocol without a warning", () => {
    expect(parseEnvelope(ok({}, UMX_PROTOCOL_MIN), false, "").warnings).toEqual([]);
    expect(parseEnvelope(ok({}, UMX_PROTOCOL), false, "").warnings).toEqual([]);
  });

  it("reads a newer protocol with a ui_protocol_newer warning instead of refusing", () => {
    const env = parseEnvelope<{ n: number }>(ok({ n: 1 }, UMX_PROTOCOL + 1), false, "");
    expect(env.data).toEqual({ n: 1 });
    expect(env.warnings.map((w) => w.code)).toEqual(["ui_protocol_newer"]);
  });

  it("reads an older umx's 0-based flags from 1 (F217) on each record umx flags, and leaves current ones as they are", () => {
    type Flags = { secret_suspected: string[] };
    type Flagged = Flags & { node: Flags; rows: Flags[]; members: Flags[]; proposed: Flags[] };
    const data = (): Flagged => ({
      secret_suspected: ["key:0"],
      node: { secret_suspected: ["field:note", "key:0", "evidence:2", "status_evidence:1", "evidence_removed:0", "shape:0"] },
      rows: [{ secret_suspected: ["status_reason:9"] }],
      members: [{ secret_suspected: ["key:1"] }],
      proposed: [{ secret_suspected: ["field:title"] }],
    });
    const old = parseEnvelope<Flagged>(ok(data(), FLAGS_FROM_ONE - 1), false, "");
    expect(old.data.secret_suspected).toEqual(["key:1"]);
    expect(old.data.node.secret_suspected).toEqual(["field:note", "key:1", "evidence:3", "status_evidence:2", "evidence_removed:1", "shape:0"]);
    expect(old.data.rows[0].secret_suspected).toEqual(["status_reason:10"]);
    expect(old.data.members[0].secret_suspected).toEqual(["key:2"]);
    expect(old.data.proposed[0].secret_suspected).toEqual(["field:title"]);
    expect(parseEnvelope(ok(data(), FLAGS_FROM_ONE), false, "").data).toEqual(data());
  });

  it("never rewrites graph content that holds a secret_suspected key below a record (review on #385)", () => {
    type Opaque = { payload: { secret_suspected: string[] }; list: { secret_suspected: string[] }[] };
    type Replied = { node: Opaque & { secret_suspected: string[] }; rows: Opaque[] };
    const opaque = (): Opaque => ({ payload: { secret_suspected: ["evidence:0"] }, list: [{ secret_suspected: ["key:0"] }] });
    const data = (): Replied => ({ node: { ...opaque(), secret_suspected: ["key:0"] }, rows: [opaque()] });
    const old = parseEnvelope<Replied>(ok(data(), FLAGS_FROM_ONE - 1), false, "");
    expect(old.data.node).toEqual({ ...opaque(), secret_suspected: ["key:1"] });
    expect(old.data.rows[0]).toEqual(opaque());
  });

  it("reads status warning as a success that carries its warnings (umx docs get output)", () => {
    const body = JSON.stringify({
      umx: UMX_PROTOCOL, command: "search", status: "warning", data: { rows: [] }, errors: [],
      warnings: [{ code: "similarity_unavailable", message: "Similarity was not used.", next: "umx docs get search" }],
    });
    expect(parseEnvelope(body, false, "").warnings.map((w) => w.code)).toEqual(["similarity_unavailable"]);
  });

  it("fails a reply that carries errors even when its status says warning (R199-2)", () => {
    const body = JSON.stringify({
      umx: UMX_PROTOCOL, command: "search", status: "warning", data: { rows: [] },
      warnings: [{ code: "similarity_unavailable", message: "Similarity was not used.", next: null }],
      errors: [{ code: "graph_not_loaded", message: "demo is not loaded.", next: "umx graph load <folder>" }],
    });
    expect(() => parseEnvelope(body, false, "")).toThrow(
      expect.objectContaining({ code: "graph_not_loaded", message: "demo is not loaded.", next: "umx graph load <folder>" }),
    );
  });

  it("keeps execFile's failure text (it holds the argv and the search text) out of the reply", async () => {
    const detail = "Command failed: /opt/umx/bin/umx search secret words --graph demo --limit 20";
    const exec = vi.fn<Exec>(async () => ({ stdout: "", failed: true, detail }));
    const read = createUmx({ exec, bin: () => "/opt/umx/bin/umx", env: () => ({}) });
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const e = await refused(read("search", { graph: "demo", query: "secret words" }));
    expect(e.code).toBe("ui_umx_output");
    expect(JSON.stringify(e.toJSON())).not.toMatch(/secret|Command failed|\/opt\/umx/);
    expect(e.message).toBe("umx failed; see the view's terminal.");
    expect(logged).toHaveBeenCalledWith(expect.stringContaining("secret words"));
    logged.mockRestore();
  });

  it("passes umx's own error through as given", () => {
    const body = JSON.stringify({
      umx: UMX_PROTOCOL, command: "node get", status: "error", data: null, warnings: [],
      errors: [{ code: "node_not_found", message: "No node 01Z... in demo.", next: "umx node find <words>" }],
    });
    expect(() => parseEnvelope(body, true, "")).toThrow(expect.objectContaining({ code: "node_not_found", next: "umx node find <words>" }));
  });

  it("resolves umx to an absolute path and gives it only UMX_HOME and UMX_OUTPUT", () => {
    expect(resolveBin({ UMX_BIN: "/x/umx" }, () => null)).toBe("/x/umx");
    expect(() => resolveBin({ UMX_BIN: "umx" }, () => null)).toThrow(UmxError);
    expect(resolveBin({ PATH: "/a" }, () => "/a/umx")).toBe("/a/umx");
    expect(() => resolveBin({ PATH: "/a" }, () => null)).toThrow(UmxError);
    expect(umxEnv({ UMX_HOME: "/h", UMX_UI_TOKEN: "secret", PATH: "/bin" })).toEqual({ UMX_HOME: "/h", UMX_OUTPUT: "json" });
  });
});

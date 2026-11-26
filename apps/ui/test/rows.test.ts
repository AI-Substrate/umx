// The rows a live change touched (lib/rows.ts, plan 127): edges read by id, a few reads, never one per id; nodes with
// umx's secret flags (ws009 M2b), a node get each for a few, else the graph's node list.
import { describe, expect, it, vi } from "vitest";
import { POST } from "@/app/api/graphs/[alias]/rows/route";
import { BODY_MAX } from "@/lib/http";
import { createRows, EDGE_CHUNK, readWanted, ROWS_MAX } from "@/lib/rows";
import { createUmx, type Exec, type ReadArgs, UMX_PROTOCOL, UmxError } from "@/lib/umx";

const id = (i: number) => `01${String(i).padStart(24, "0")}`;
const JSON_TYPE = "application/json";

function harness(edgeLimit = Infinity, busy = 0) {
  const calls: { verb: string; ids: readonly string[] }[] = [];
  let busyLeft = busy;
  const read = vi.fn(async (verb: string, a: ReadArgs = {}) => {
    if (verb === "ontology show") return { umx: 48, command: verb, status: "ok" as const, data: { symmetric: ["conflicts_with", "rivals"] }, warnings: [], errors: [] };
    calls.push({ verb, ids: a.ids ?? (a.id ? [a.id] : []) });
    // A node as umx's flagged reads give it: id(13) holds a secret-looking title, flagged (F188).
    // id(14) carries a flag the view does not know: its title is hidden too (review N1).
    const flags = (x: string) => (x === id(13) ? ["field:title"] : x === id(14) ? ["shape:0"] : null);
    const node = (x: string) => ({ id: x, type: "risk", title: x === id(13) || x === id(14) ? "s3cret" : "t", status: "open", entity_key: ["email:a@x.invalid"], ...(flags(x) ? { secret_suspected: flags(x) } : {}) });
    if (verb === "node row") {
      if (a.id === id(999)) throw new UmxError("node_not_found", "no such node");
      return { umx: 63, command: "node get", status: "ok" as const, data: { node: node(a.id!) }, warnings: [], errors: [] };
    }
    if (verb === "node list") {
      const all = Array.from({ length: 800 }, (_v, i) => node(id(i)));
      return { umx: 63, command: "node list", status: "ok" as const, data: { rows: all.slice(a.offset ?? 0), total: all.length }, warnings: [], errors: [] };
    }
    if (busyLeft-- > 0) throw new UmxError("query_busy", "busy");
    const ids = a.ids ?? [];
    if (verb === "edges by id" && ids.length > edgeLimit) throw new UmxError("query_too_large", "work limit");
    const rows = ids.map((x, i) => ({ id: x, type: "blocks", from: id(1), to: id(2), from_graph: "demo", to_graph: i === 0 && x === id(777) ? "ops" : "demo" }));
    return { umx: 47, command: "query", status: "ok" as const, data: { rows }, warnings: [], errors: [] };
  });
  const check = vi.fn(async (g: string) => {
    if (g === "vault") throw new UmxError("ui_graph_not_open", "no", null, 409);
  });
  const rows = createRows({ read: read as never, check, sleep: async () => {} });
  return { rows, read, check, calls };
}

describe("the row request", () => {
  it("takes full ids only, each once, at most ROWS_MAX of a kind, as JSON", () => {
    expect(readWanted(JSON_TYPE, JSON.stringify({ nodes: [id(1), id(1)], edges: [id(2)] }))).toEqual({ nodes: [id(1)], edges: [id(2)] });
    expect(() => readWanted("text/plain", "{}")).toThrow(expect.objectContaining({ code: "ui_bad_rows" }));
    expect(() => readWanted(JSON_TYPE, "nope")).toThrow(expect.objectContaining({ code: "ui_bad_rows" }));
    expect(() => readWanted(JSON_TYPE, JSON.stringify({ nodes: ["--graph"] }))).toThrow(expect.objectContaining({ code: "ui_bad_id" }));
    const many = Array.from({ length: ROWS_MAX + 1 }, (_v, i) => id(i));
    expect(() => readWanted(JSON_TYPE, JSON.stringify({ edges: many }))).toThrow(expect.objectContaining({ code: "ui_bad_rows" }));
  });
});

describe("reading rows by id", () => {
  it("carries declarations and local/external attribution without another daemon read", async () => {
    const hubs = [{ name: "register", form: "named", node: id(1), via: "part_of" }];
    const exec = vi.fn<Exec>(async (_bin, argv) => {
      const ontology = argv[0] === "ontology";
      const rows = [1, 2, 3].map((n) => ({
        id: id(n), owner: "demo", type: "part_of", from: id(4), to: id(1), from_graph: "demo", to_graph: n === 3 ? "ops" : "demo",
        ...(n > 1 ? { auto_added_by: "register" } : {}),
      }));
      return { stdout: JSON.stringify({ umx: UMX_PROTOCOL, command: ontology ? "ontology show" : "query", status: "ok", data: ontology ? { symmetric: [], hubs } : { rows }, warnings: [], errors: [] }), failed: false, detail: "" };
    });
    const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
    const got = await createRows({ read, check: async () => {}, sleep: async () => {} })("demo", { nodes: [], edges: [id(1), id(2), id(3)], with: ["ops"] });
    expect(got.hubs).toEqual(hubs);
    expect(got.edges).toEqual([[id(1), "part_of", id(4), id(1)], [id(2), "part_of", id(4), id(1), "register"]]);
    expect(got.external).toEqual([[id(3), "part_of", "demo", id(4), "ops", id(1), "register"]]);
    expect(exec).toHaveBeenCalledTimes(2);
  });

  it("checks the graph with the picker's rule first, and reads nothing for one it would not open", async () => {
    const h = harness();
    await expect(h.rows("vault", { nodes: [id(1)], edges: [] })).rejects.toMatchObject({ code: "ui_graph_not_open" });
    expect(h.read).not.toHaveBeenCalled();
  });

  it("reads a few nodes one flagged node get each, more from the graph's node list, and leaves out an id umx did not return", async () => {
    const h = harness();
    const few = await h.rows("demo", { nodes: [id(0), id(1), id(999)], edges: [] });
    expect(h.calls.map((c) => [c.verb, c.ids])).toEqual([["node row", [id(0)]], ["node row", [id(1)]], ["node row", [id(999)]]]);
    expect(few.nodes).toEqual([[id(0), "risk", "t", "open", "", ["email:a@x.invalid"]], [id(1), "risk", "t", "open", "", ["email:a@x.invalid"]]]);
    h.calls.length = 0;
    const nodes = [...Array.from({ length: 700 }, (_v, i) => id(i)), id(999)];
    const got = await h.rows("demo", { nodes, edges: [] });
    expect(h.calls.map((c) => c.verb)).toEqual(["node list"]);
    expect(got.nodes).toHaveLength(700);
    expect(got.nodes[0]).toEqual([id(0), "risk", "t", "open", "", ["email:a@x.invalid"]]);
  });

  it("sends a title umx flags as its type and short id, from either read (ws009 M2b)", async () => {
    const h = harness();
    for (const nodes of [[id(13), id(14)], Array.from({ length: 30 }, (_v, i) => id(i))]) {
      const got = await h.rows("demo", { nodes, edges: [] });
      expect(got.nodes.find((n) => n[0] === id(13))![2]).toBe("risk · 000013");
      expect(got.nodes.find((n) => n[0] === id(14))![2]).toBe("risk · 000014");
      expect(JSON.stringify(got)).not.toContain("s3cret");
    }
  });

  it("halves an edge read on query_too_large, keeps the size that fitted for the graph, and keeps only edges drawn here", async () => {
    const h = harness(30);
    const edges = [id(777), ...Array.from({ length: 59 }, (_v, i) => id(i))];
    const got = await h.rows("demo", { nodes: [], edges });
    expect(EDGE_CHUNK).toBeGreaterThan(60);
    expect(h.calls.map((c) => c.ids.length)).toEqual([60, 30, 30]);
    expect(got.edges.map((e) => e[0])).not.toContain(id(777));
    expect(got.edges).toHaveLength(59);
    h.calls.length = 0;
    await h.rows("demo", { nodes: [], edges: edges.slice(0, 31) });
    expect(h.calls.map((c) => c.ids.length)).toEqual([30, 1]);
  });

  it("names the other open graphs in the edge read, returns an edge between them as external, and checks each first (plan 133)", async () => {
    const h = harness();
    const r = await h.rows("demo", { nodes: [], edges: [id(777), id(5)], with: ["ops"] });
    expect(h.check.mock.calls.map((c) => c[0])).toEqual(["demo", "ops"]);
    expect(h.read.mock.calls[0][1]).toMatchObject({ graph: "demo", with: ["ops"] });
    expect(r.edges.map((e) => e[0])).toEqual([id(5)]);
    expect(r.external).toEqual([[id(777), "blocks", "demo", id(1), "ops", id(2)]]);
    // Without ops open the same edge is not drawn here, so it reads as gone; and a graph the picker would not open
    // is refused before anything is read.
    expect((await harness().rows("demo", { nodes: [], edges: [id(777)] })).edges).toEqual([]);
    const v = harness();
    await expect(v.rows("demo", { nodes: [], edges: [id(5)], with: ["vault"] })).rejects.toMatchObject({ code: "ui_graph_not_open" });
    expect(v.read).not.toHaveBeenCalled();
    expect(readWanted(JSON_TYPE, JSON.stringify({ edges: [id(5)], with: ["ops", "ops"] }))).toEqual({ nodes: [], edges: [id(5)], with: ["ops"] });
    expect(() => readWanted(JSON_TYPE, JSON.stringify({ with: ["--graph"] }))).toThrow(expect.objectContaining({ code: "ui_bad_graph" }));
    expect(() => readWanted(JSON_TYPE, JSON.stringify({ with: Array.from({ length: 12 }, (_v, i) => `g${i}`) }))).toThrow(expect.objectContaining({ code: "ui_bad_rows" }));
  });

  it("never takes an equal edge id that another open graph stores as this graph's edge (review B2)", async () => {
    const read = vi.fn(async (verb: string, a: ReadArgs = {}) => {
      // beta stores an edge with the same id between its own nodes; alpha's edge with that id is not drawable here.
      const rows = verb === "edges by id" ? (a.ids ?? []).map((x) => ({ id: x, owner: "beta", type: "blocks", from: id(1), to: id(2), from_graph: "beta", to_graph: "beta" })) : [];
      return { umx: 48, command: "query", status: "ok" as const, data: { rows }, warnings: [], errors: [] };
    });
    const rows = createRows({ read: read as never, check: async () => {}, sleep: async () => {} });
    const r = await rows("alpha", { nodes: [], edges: [id(5)], with: ["beta"] });
    expect(r.edges).toEqual([]);
    expect(r.external).toEqual([]);
  });

  it("tries again when umx is already running two queries", async () => {
    const h = harness(Infinity, 2);
    const got = await h.rows("demo", { nodes: [], edges: [id(1)] });
    expect(got.edges).toHaveLength(1);
    // Two busy answers, the read that worked, and the graph's symmetric relations.
    expect(h.read).toHaveBeenCalledTimes(4);
  });

  it("brings the graph's symmetric relations with every reply, a request with no ids too (plan 150 review B1)", async () => {
    const h = harness();
    const got = await h.rows("demo", { nodes: [], edges: [] });
    expect(got).toEqual({ graph: "demo", nodes: [], edges: [], symmetric: ["conflicts_with", "rivals"] });
    expect(h.read.mock.calls.map((c) => [c[0], c[1]?.graph])).toEqual([["ontology show", "demo"]]);
    expect((await h.rows("demo", { nodes: [id(1)], edges: [], with: ["ops"] })).symmetric).toEqual(["conflicts_with", "rivals"]);
  });
});

describe("the rows route's body cap (lib/http readBody, security)", () => {
  const params = { params: Promise.resolve({ alias: "demo" }) };
  const url = "http://127.0.0.1:4321/api/graphs/demo/rows";

  it("refuses a declared body over the cap before reading it", async () => {
    const res = await POST(new Request(url, { method: "POST", headers: { "content-type": JSON_TYPE, "content-length": String(BODY_MAX + 1) }, body: "{}" }), params);
    expect(res.status).toBe(413);
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("ui_body_too_big");
  });

  it("refuses a chunked body past the cap, reading no further than the cap", async () => {
    const big = chunked(64, 4096);
    const res = await POST(new Request(url, { method: "POST", headers: { "content-type": JSON_TYPE }, body: big.stream, duplex: "half" } as RequestInit), params);
    expect(res.status).toBe(413);
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("ui_body_too_big");
    expect(big.sent()).toBeLessThanOrEqual(BODY_MAX + 2 * 4096);
    expect(big.cancelled()).toBe(true);
  });
});

/** A body with no Content-Length: `count` chunks of `size` bytes, sent only as the reader asks for them. */
function chunked(count: number, size: number) {
  let sent = 0;
  let cancelled = false;
  let n = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(c) {
      if (n++ >= count) return c.close();
      sent += size;
      c.enqueue(new Uint8Array(size).fill(0x20));
    },
    cancel() {
      cancelled = true;
    },
  });
  return { stream, sent: () => sent, cancelled: () => cancelled };
}

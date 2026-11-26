// A snapshot reads edges by name (EDGE_COLUMNS): only their ends and where they are, never titles or sentences. A
// column a row leaves out comes back null, which is read as absent, so edges land where a full row put them.
import { describe, expect, it, vi } from "vitest";
import { edgePlace, loadSnapshot, ontologyOf } from "@/lib/snapshot";
import { createUmx, EDGE_COLUMNS, UMX_PROTOCOL, type Exec } from "@/lib/umx";

const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA";
const GB = "01GRAPHBBBBBBBBBBBBBBBBBBB";
const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const env = (command: string, data: unknown) => JSON.stringify({ umx: UMX_PROTOCOL, command, status: "ok", data, warnings: [], errors: [] });
const edge = (id: string, more: object) => ({ id, type: "relates_to", from: A, to: B, graph_id: GA, from_graph: GA, to_graph: GA, from_state: null, to_state: null, ...more });

describe("a snapshot's edges", () => {
  it("are read by name, and placed the same whether a column is null or left out", async () => {
    const rows = [edge("E1", { auto_added_by: "register" }), edge("E2", { to_graph: GB, auto_added_by: "register" }), edge("E3", { to_state: "graph_not_loaded", to_graph: GB }), edge("E4", { to_state: "dangling" })];
    const exec = vi.fn<Exec>(async (_bin, argv) => {
      const verb = `${argv[0]} ${argv[1]}`;
      if (verb === "graph list") return { stdout: env(verb, { rows: [{ alias: "demo", id: GA, path: "/h/g/demo", state: "loaded", folder: "present", policy: "open" }] }), failed: false, detail: "" };
      if (verb === "edge list") return { stdout: env(verb, { rows, total: rows.length, truncated: false }), failed: false, detail: "" };
      if (verb === "node list") return { stdout: env(verb, { rows: [], total: 0, truncated: false }), failed: false, detail: "" };
      return { stdout: env(verb, {}), failed: false, detail: "" };
    });
    const snap = await loadSnapshot(createUmx({ exec, bin: () => "/u", env: () => ({}) }), "demo");
    const edgeCall = exec.mock.calls.find((c) => c[1][0] === "edge")![1];
    expect(edgeCall.slice(-2)).toEqual(["--columns", EDGE_COLUMNS]);
    expect(snap.edges.map((e) => e[0])).toEqual(["E1"]);
    expect(snap.edges[0][4]).toBe("register");
    expect(snap.external?.[0][7]).toBe("register");
    expect((snap.external ?? []).map((e) => e[0])).toEqual(["E2", "E3"]);
    expect(snap.broken).toEqual(["E4"]);
    const bare: Record<string, unknown> = edge("E5", {});
    delete bare.from_state;
    delete bare.to_state;
    expect(edgePlace(bare as Parameters<typeof edgePlace>[0], GA)).toBe(edgePlace(edge("E5", {}), GA));
  });

  it("uses the existing ontology read for named/type hubs and distinguishes no declarations from an old daemon", async () => {
    for (const hubs of [undefined, [], [
      { name: "register", form: "named", node: A, via: "part_of", match: { type: ["risk"] } },
      { name: "meetings", form: "type", type: "meeting", via: "raised_in" },
    ]]) {
      const exec = vi.fn<Exec>(async () => ({ stdout: env("ontology show", { symmetric: ["related"], ...(hubs === undefined ? {} : { hubs }) }), failed: false, detail: "" }));
      const read = createUmx({ exec, bin: () => "/u", env: () => ({}) });
      const result = await ontologyOf(read, "demo");
      expect(result.symmetric).toEqual(["related"]);
      if (hubs === undefined) expect(result).not.toHaveProperty("hubs");
      else expect(result.hubs).toEqual(hubs.map((row) => row.form === "named" ? { name: row.name, form: row.form, via: row.via, node: row.node } : row));
      expect(exec).toHaveBeenCalledTimes(1);
      expect(exec.mock.calls[0][1].slice(0, 2)).toEqual(["ontology", "show"]);
    }
  });

  it("surfaces malformed declaration data instead of silently enabling the legacy preview", async () => {
    for (const hubs of [null, {}, [{ name: "x", form: "named", node: "short", via: "part_of" }], [{ name: "x", form: "type", type: "meeting" }]]) {
      const exec: Exec = async () => ({ stdout: env("ontology show", { hubs }), failed: false, detail: "" });
      await expect(ontologyOf(createUmx({ exec, bin: () => "/u", env: () => ({}) }), "demo")).rejects.toMatchObject({ code: "ui_bad_hubs" });
    }
  });
});

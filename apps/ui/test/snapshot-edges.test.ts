// A snapshot reads edges by name (EDGE_COLUMNS): only their ends and where they are, never titles or sentences. A
// column a row leaves out comes back null, which is read as absent, so edges land where a full row put them.
import { describe, expect, it, vi } from "vitest";
import { edgePlace, loadSnapshot } from "@/lib/snapshot";
import { createUmx, EDGE_COLUMNS, UMX_PROTOCOL, type Exec } from "@/lib/umx";

const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA";
const GB = "01GRAPHBBBBBBBBBBBBBBBBBBB";
const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const env = (command: string, data: unknown) => JSON.stringify({ umx: UMX_PROTOCOL, command, status: "ok", data, warnings: [], errors: [] });
const edge = (id: string, more: object) => ({ id, type: "relates_to", from: A, to: B, graph_id: GA, from_graph: GA, to_graph: GA, from_state: null, to_state: null, ...more });

describe("a snapshot's edges", () => {
  it("are read by name, and placed the same whether a column is null or left out", async () => {
    const rows = [edge("E1", {}), edge("E2", { to_graph: GB }), edge("E3", { to_state: "graph_not_loaded", to_graph: GB }), edge("E4", { to_state: "dangling" })];
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
    expect((snap.external ?? []).map((e) => e[0])).toEqual(["E2", "E3"]);
    expect(snap.broken).toEqual(["E4"]);
    const bare: Record<string, unknown> = edge("E5", {});
    delete bare.from_state;
    delete bare.to_state;
    expect(edgePlace(bare as Parameters<typeof edgePlace>[0], GA)).toBe(edgePlace(edge("E5", {}), GA));
  });
});

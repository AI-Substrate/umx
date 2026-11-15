// Publishing the selection on the server (lib/publish.ts, app/api/graphs/[alias]/selection, plan 117): the body is
// read strictly, the graph passes the picker's check before anything reaches the daemon, and nothing else is sent.
import { describe, expect, it, vi } from "vitest";
import { POST } from "@/app/api/graphs/[alias]/selection/route";
import { BODY_MAX, readBody } from "@/lib/http";
import { createPublish, readSelection } from "@/lib/publish";
import { UmxError } from "@/lib/umx";

const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const JSON_TYPE = "application/json";

function rig(check: (g: string) => Promise<void> = async () => {}) {
  const calls: string[] = [];
  const select = vi.fn(async (graph: string, nodes: string[]) => void calls.push(`select ${graph} ${nodes.join(",")}`));
  const publish = createPublish({ check: async (g) => (calls.push(`check ${g}`), check(g)), select });
  return { publish, select, calls };
}

async function refused(p: Promise<unknown>, code: string, status: number) {
  await expect(p).rejects.toSatisfy((e: unknown) => e instanceof UmxError && e.code === code && e.httpStatus === status);
}

describe("publishing the selection", () => {
  it("checks the graph, then sends its nodes once each", async () => {
    const r = rig();
    expect(await r.publish("demo", JSON_TYPE, JSON.stringify({ nodes: [A, B, A] }))).toEqual({ graph: "demo", selected: 2 });
    expect(r.calls).toEqual(["check demo", `select demo ${A},${B}`]);
  });

  it("sends an empty selection, which clears the daemon's copy", async () => {
    const r = rig();
    await r.publish("demo", "application/json; charset=utf-8", '{"nodes":[]}');
    expect(r.select).toHaveBeenCalledWith("demo", []);
  });

  it("refuses a graph the picker would not open, before the daemon hears of it", async () => {
    const r = rig(async () => {
      throw new UmxError("ui_graph_not_open", "secured", null, 409);
    });
    await refused(r.publish("vault", JSON_TYPE, JSON.stringify({ nodes: [A] })), "ui_graph_not_open", 409);
    expect(r.select).not.toHaveBeenCalled();
  });

  it("refuses a bad alias, a bad body, a non-id, too many nodes and a non-JSON content type", async () => {
    const r = rig();
    await refused(r.publish("../x", JSON_TYPE, '{"nodes":[]}'), "ui_bad_graph", 400);
    await refused(r.publish("demo", "text/plain", JSON.stringify({ nodes: [A] })), "ui_bad_selection", 415);
    await refused(r.publish("demo", null, JSON.stringify({ nodes: [A] })), "ui_bad_selection", 415);
    for (const body of ["nope", "null", '{"nodes":"x"}', "[]"]) await refused(r.publish("demo", JSON_TYPE, body), "ui_bad_selection", 400);
    for (const n of ["--graph", 7, null, `${A}x`]) await refused(r.publish("demo", JSON_TYPE, JSON.stringify({ nodes: [A, n] })), "ui_bad_id", 400);
    expect(() => readSelection(JSON.stringify({ nodes: Array.from({ length: 1001 }, () => A) }))).toThrow(/at most 1000/);
    expect(readSelection(JSON.stringify({ nodes: Array.from({ length: 1000 }, () => A) }))).toEqual([A]);
    expect(() => readSelection(" ".repeat(BODY_MAX + 1))).toThrow(/too big/);
    expect(r.select).not.toHaveBeenCalled();
  });

  it("the route refuses an oversized body before reading it", async () => {
    const res = await POST(
      new Request("http://127.0.0.1:4321/api/graphs/demo/selection", {
        method: "POST",
        headers: { "content-type": JSON_TYPE, "content-length": String(BODY_MAX + 1) },
        body: "{}",
      }),
      { params: Promise.resolve({ alias: "demo" }) },
    );
    expect(res.status).toBe(413);
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("ui_body_too_big");
  });

  it("the route refuses a chunked body past the cap, reading no further than the cap", async () => {
    const big = chunked(64, 4096);
    const res = await POST(new Request("http://127.0.0.1:4321/api/graphs/demo/selection", { method: "POST", headers: { "content-type": JSON_TYPE }, body: big.stream, duplex: "half" } as RequestInit), {
      params: Promise.resolve({ alias: "demo" }),
    });
    expect(res.status).toBe(413);
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("ui_body_too_big");
    expect(big.sent()).toBeLessThanOrEqual(BODY_MAX + 2 * 4096);
    expect(big.cancelled()).toBe(true);
  });
});

/** A body with no Content-Length: `count` chunks of `size` bytes, sent only as the reader asks for them. */
function chunked(count: number, size: number, byte = 0x20) {
  let sent = 0;
  let cancelled = false;
  let n = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull(c) {
      if (n++ >= count) return c.close();
      sent += size;
      c.enqueue(new Uint8Array(size).fill(byte));
    },
    cancel() {
      cancelled = true;
    },
  });
  return { stream, sent: () => sent, cancelled: () => cancelled };
}

describe("reading a request body", () => {
  const req = (body: BodyInit, headers: Record<string, string> = {}) => new Request("http://127.0.0.1:4321/x", { method: "POST", headers, body, duplex: "half" } as RequestInit);

  it("reads a chunked body under the cap whole, a character split across chunks included", async () => {
    const bytes = new TextEncoder().encode(`{"t":"é${"x".repeat(5000)}"}`);
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(bytes.slice(0, 7));
        c.enqueue(bytes.slice(7));
        c.close();
      },
    });
    expect(JSON.parse(await readBody(req(stream)))).toEqual({ t: `é${"x".repeat(5000)}` });
    expect(await readBody(req(chunked(16, 4096).stream))).toHaveLength(BODY_MAX);
  });

  it("refuses one byte past the cap, chunked or declared", async () => {
    await expect(readBody(req(chunked(17, 4096).stream))).rejects.toMatchObject({ code: "ui_body_too_big", httpStatus: 413 });
    await expect(readBody(req("{}", { "content-length": String(BODY_MAX + 1) }))).rejects.toMatchObject({ code: "ui_body_too_big" });
    await expect(readBody(req(chunked(1, BODY_MAX + 1).stream))).rejects.toMatchObject({ code: "ui_body_too_big" });
  });

  it("streams: stops pulling at the cap and cancels the body, never reading the rest (security)", async () => {
    const big = chunked(1024, 4096);
    await expect(readBody(req(big.stream))).rejects.toMatchObject({ code: "ui_body_too_big", httpStatus: 413 });
    // 4 MiB offered; the reader stops within a chunk or two (one the stream may have queued ahead) past 64 KiB.
    expect(big.sent()).toBeLessThanOrEqual(BODY_MAX + 2 * 4096);
    expect(big.cancelled()).toBe(true);
  });
});

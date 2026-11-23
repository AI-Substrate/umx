// POST /api/launch takes a launch code umx armed: once, within its minute. It sets no cookie: the view has none.
import { NextRequest } from "next/server";
import { describe, expect, it } from "vitest";
import { POST } from "@/app/api/launch/route";
import { armCode, launchState } from "@/bin/launch-codes.mjs";

function post(body: string) {
  return POST(
    new NextRequest("http://127.0.0.1:4321/api/launch", {
      method: "POST",
      headers: { "content-type": "application/json", origin: "http://127.0.0.1:4321" },
      body,
    }),
  );
}

describe("the launch code route", () => {
  // ws009 M1b2 (security D5): a launch code umx armed is taken once; used, expired or wrong, it is refused, and a
  // wrong one uses up no other.
  it("takes a launch code once, within its minute, and a wrong one uses up no other", async () => {
    const state = launchState();
    const a = "A".repeat(42) + "a";
    const b = "B".repeat(42) + "b";
    expect(armCode(state, a, Date.now()) && armCode(state, b, Date.now())).toBe(true);
    expect((await post(JSON.stringify({ code: "C".repeat(43) }))).status).toBe(401);
    const first = await post(JSON.stringify({ code: a }));
    expect(first.status).toBe(204);
    expect(first.headers.get("set-cookie")).toBeNull();
    expect((await post(JSON.stringify({ code: a }))).status).toBe(401);
    expect((await post(JSON.stringify({ code: b }))).status).toBe(204);
    const old = "D".repeat(43);
    armCode(state, old, Date.now() - 60_001);
    expect((await post(JSON.stringify({ code: old }))).status).toBe(401);
  });

  it("refuses a missing, malformed or non-string code", async () => {
    for (const body of [JSON.stringify({ code: 7 }), JSON.stringify({}), "not json", JSON.stringify({ code: "x".repeat(42) })]) {
      expect((await post(body)).status).toBe(401);
    }
  });

  it("refuses a chunked body past the cap, reading no further than the cap", async () => {
    let n = 0;
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      pull(c) {
        if (n++ >= 64) return c.close();
        c.enqueue(new Uint8Array(4096).fill(0x20));
      },
      cancel() {
        cancelled = true;
      },
    });
    const res = await POST(
      new NextRequest("http://127.0.0.1:4321/api/launch", {
        method: "POST",
        headers: { "content-type": "application/json", origin: "http://127.0.0.1:4321" },
        body,
        duplex: "half",
      } as ConstructorParameters<typeof NextRequest>[1]),
    );
    expect(res.status).toBe(413);
    // 256 KiB offered with no Content-Length; reading stops within a chunk or two past 64 KiB, and the body is cancelled.
    expect(n * 4096).toBeLessThanOrEqual(64 * 1024 + 2 * 4096);
    expect(cancelled).toBe(true);
  });
});

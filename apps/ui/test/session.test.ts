// POST /api/session swaps the launch link's token for the session cookie. A wrong or stale token must never clear or
// replace the cookie a tab already holds (security N2).
import { NextRequest } from "next/server";
import { describe, expect, it } from "vitest";
import { POST } from "@/app/api/session/route";
import { armCode, launchState } from "@/bin/launch-codes.mjs";
import { RUN_KEY } from "@/lib/guard";

const TOKEN = "t".repeat(43);
const SESSION = "s".repeat(43);
const COOKIE = "umx_ui_ab12";
const run = globalThis as Record<symbol, unknown>;
run[RUN_KEY] = { UMX_UI_PORT: "4321", UMX_UI_TOKEN: TOKEN, UMX_UI_SESSION: SESSION, UMX_UI_COOKIE: COOKIE };

function post(body: string, cookie = `${COOKIE}=${SESSION}`) {
  return POST(
    new NextRequest("http://127.0.0.1:4321/api/session", {
      method: "POST",
      headers: { "content-type": "application/json", origin: "http://127.0.0.1:4321", cookie },
      body,
    }),
  );
}

describe("the token exchange route", () => {
  it("sets the session cookie for this run's token", async () => {
    const res = await post(JSON.stringify({ token: TOKEN }), "");
    expect(res.status).toBe(204);
    expect(res.headers.get("set-cookie")).toContain(`${COOKIE}=${SESSION}`);
    expect(res.headers.get("set-cookie")).toMatch(/HttpOnly/i);
  });

  it("leaves an existing session alone for a wrong, stale, short or missing token", async () => {
    for (const body of [
      JSON.stringify({ token: "x".repeat(43) }),
      JSON.stringify({ token: TOKEN.slice(0, 42) }),
      JSON.stringify({ token: `${TOKEN}x` }),
      JSON.stringify({ token: 7 }),
      JSON.stringify({}),
      "not json",
    ]) {
      const res = await post(body);
      expect(res.status).toBe(401);
      expect(res.headers.get("set-cookie")).toBeNull();
    }
  });

  // ws009 M1b2 (security D5): a launch code umx armed is swapped once; used, expired or wrong, it is refused, and a
  // wrong one uses up no other.
  it("takes a launch code once, within its minute, and a wrong one uses up no other", async () => {
    const state = launchState();
    const a = "A".repeat(42) + "a";
    const b = "B".repeat(42) + "b";
    expect(armCode(state, a, Date.now()) && armCode(state, b, Date.now())).toBe(true);
    const wrong = await post(JSON.stringify({ token: "C".repeat(43) }), "");
    expect(wrong.status).toBe(401);
    const first = await post(JSON.stringify({ token: a }), "");
    expect(first.status).toBe(204);
    expect(first.headers.get("set-cookie")).toContain(`${COOKIE}=${SESSION}`);
    expect(first.headers.get("set-cookie")).toMatch(/SameSite=Strict/i);
    const again = await post(JSON.stringify({ token: a }), "");
    expect(again.status).toBe(401);
    expect(again.headers.get("set-cookie")).toBeNull();
    expect((await post(JSON.stringify({ token: b }), "")).status).toBe(204);
    const old = "D".repeat(43);
    armCode(state, old, Date.now() - 60_001);
    expect((await post(JSON.stringify({ token: old }), "")).status).toBe(401);
  });

  it("refuses a chunked body past the cap without setting a cookie, reading no further than the cap", async () => {
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
      new NextRequest("http://127.0.0.1:4321/api/session", {
        method: "POST",
        headers: { "content-type": "application/json", origin: "http://127.0.0.1:4321" },
        body,
        duplex: "half",
      } as ConstructorParameters<typeof NextRequest>[1]),
    );
    expect(res.status).toBe(413);
    expect(res.headers.get("set-cookie")).toBeNull();
    // 256 KiB offered with no Content-Length; reading stops within a chunk or two past 64 KiB, and the body is cancelled.
    expect(n * 4096).toBeLessThanOrEqual(64 * 1024 + 2 * 4096);
    expect(cancelled).toBe(true);
  });
});

// The proxy wires lib/guard.ts to real requests: a plain same-origin visit needs no token, and refusals carry no
// detail and no CORS header.
import { NextRequest } from "next/server";
import { describe, expect, it } from "vitest";
import { RUN_KEY } from "@/lib/guard";
import { proxy } from "@/proxy";

(globalThis as Record<symbol, unknown>)[RUN_KEY] = { UMX_UI_PORT: "4321", UMX_UI_DEV: "0" };

function visit(path: string, headers: Record<string, string>, method = "GET") {
  return proxy(new NextRequest(`http://127.0.0.1:4321${path}`, { method, headers }));
}

describe("the proxy", () => {
  it("passes a plain same-origin visit and API call with no token or cookie, and sends no CORS header", () => {
    for (const host of ["127.0.0.1:4321", "localhost:4321"]) {
      const page = visit("/", { host, "sec-fetch-site": "none" });
      expect(page.status).toBe(200);
      expect(page.headers.get("x-middleware-next")).toBe("1");
      const call = visit("/api/graphs", { host, "sec-fetch-site": "same-origin", "x-umx-view": "1" });
      expect(call.headers.get("x-middleware-next")).toBe("1");
      expect(call.headers.get("access-control-allow-origin")).toBeNull();
    }
  });

  it("refuses a rebound Host, a foreign Origin, a cross-site API read and an API call without the view header", async () => {
    const refusals = [
      visit("/", { host: "evil.example:4321" }),
      visit("/api/graphs", { host: "evil.example:4321", "x-umx-view": "1" }),
      visit("/api/graphs/demo/selection", { host: "127.0.0.1:4321", origin: "http://evil.example", "x-umx-view": "1" }, "POST"),
      visit("/api/graphs", { host: "127.0.0.1:4321", "sec-fetch-site": "cross-site", "x-umx-view": "1" }),
      visit("/api/events", { host: "127.0.0.1:4321" }),
    ];
    for (const res of refusals) {
      expect(res.status).toBe(403);
      expect(await res.text()).toBe("Refused.");
      expect(res.headers.get("access-control-allow-origin")).toBeNull();
      expect(res.headers.get("x-middleware-next")).toBeNull();
    }
  });
});

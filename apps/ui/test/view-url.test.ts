// A reload keeps the view's look in the fragment and the open graphs in the tab's storage (lib/view-url). The
// fragment only ever changes how the view looks: no graph is read from it (security N1).
import { describe, expect, it } from "vitest";
import { isLaunch, readLook, readOpen, type ViewLook, writeLook } from "@/lib/view-url";

describe("the view's look in the fragment", () => {
  it("writes only what differs from the defaults, and reads it back", () => {
    expect(writeLook({ colourBy: "type", resolution: 1, spacing: 1.6, merge: true, provenance: false, retired: false, off: { relations: [], types: [], statuses: [] } })).toBe("");
    const look: ViewLook = { colourBy: "graph", grouping: "parent", resolution: 1.4, spacing: 2.2, merge: false, provenance: true, retired: true, off: { relations: ["uses", "part_of"], types: ["topic"], statuses: [] } };
    const hash = writeLook(look);
    expect(hash).toBe("colour=graph&group=parent&res=1.4&spacing=2.2&merge=0&prov=1&retired=1&off.rel=part_of,uses&off.type=topic");
    expect(readLook(`#${hash}`)).toEqual({ ...look, off: { relations: ["part_of", "uses"], types: ["topic"], statuses: [] } });
  });

  it("drops anything unknown, out of range or oddly named, and never reads graphs", () => {
    expect(readLook("#colour=red&group=x&res=99&spacing=-1&merge=yes&off.type=<b>,ok&g=vault")).toEqual({ off: { relations: [], types: ["ok"], statuses: [] } });
    expect(readLook("")).toEqual({});
  });

  it("leaves a launch link's fragment to the launch", () => {
    expect(isLaunch("#t=abc&g=a,b")).toBe(true);
    expect(isLaunch("#g=a")).toBe(true);
    expect(isLaunch("#colour=graph")).toBe(false);
  });
});

describe("the open graphs in the tab", () => {
  it("reads a kept list, none kept on purpose, or nothing", () => {
    expect(readOpen('["live","later","live"]')).toEqual(["live", "later"]);
    expect(readOpen("[]")).toEqual([]);
    expect(readOpen(null)).toBeNull();
    expect(readOpen("{")).toBeNull();
    expect(readOpen('["a b"]')).toBeNull();
    expect(readOpen(JSON.stringify(Array.from({ length: 20 }, (_, i) => `g${i}`)))).toHaveLength(12);
  });
});

// The change feed as the view's server follows it and a tab applies it (lib/live.ts, plan 127).
import { describe, expect, it } from "vitest";
import { type DaemonChange, type Ids, MAX_CHANGE_IDS, plan, Revisions, thenIds, toRead } from "@/lib/live";

const E1 = "00000000000000e1";
const E2 = "00000000000000e2";
const ids = (added: string[] = [], edited: string[] = [], removed: string[] = []): Ids => ({ added, edited, removed });
const NONE = ids();
const step = (from: number, to: number, nodes: Ids = NONE, edges: Ids = NONE): DaemonChange => ({ graph: "demo", overflow: false, from_rev: from, to_rev: to, nodes, edges });
const overflow = (rev: number): DaemonChange => ({ graph: "demo", overflow: true, rev });
const sorted = (i: Ids) => ({ added: [...i.added].sort(), edited: [...i.edited].sort(), removed: [...i.removed].sort() });

describe("merging changes (umx-core TouchedIds::then)", () => {
  it("keeps added through an edit, drops added then removed, and reads removed then added as edited", () => {
    expect(sorted(thenIds(ids(["a"]), ids([], ["a"])))).toEqual(ids(["a"]));
    expect(sorted(thenIds(ids(["a"]), ids([], [], ["a"])))).toEqual(ids());
    expect(sorted(thenIds(ids([], [], ["a"]), ids(["a"])))).toEqual(ids([], ["a"]));
    expect(sorted(thenIds(ids([], ["a"]), ids([], [], ["a"])))).toEqual(ids([], [], ["a"]));
    expect(sorted(thenIds(ids(["a"], ["b"]), ids(["c"], ["b", "d"], ["e"])))).toEqual(ids(["a", "c"], ["b", "d"], ["e"]));
  });

  it("reads every id added or edited, never one removed", () => {
    expect(toRead({ nodes: ids(["a"], ["b"], ["c"]), edges: ids(["x"], [], ["y"]) })).toEqual({ nodes: ["a", "b"], edges: ["x"] });
  });
});

describe("the server's revisions per graph", () => {
  function followed() {
    const r = new Revisions();
    r.follow("demo");
    r.record(overflow(3));
    return r;
  }

  it("starts at 0 (an overflow from the daemon), then gives each tab what it lacks, merged", () => {
    const r = new Revisions();
    expect(r.rev("demo")).toBe(0);
    r.follow("demo");
    r.record(overflow(3));
    expect(r.rev("demo")).toBe(3);
    r.record(step(3, 4, ids(["a"])));
    r.record(step(4, 5, ids([], ["a"]), ids(["x"])));
    expect(r.since("demo", E1, { epoch: E1, rev: 5 })).toBeNull();
    expect(r.since("demo", E1, { epoch: E1, rev: 4 })).toEqual({ kind: "changed", from: 4, to: 5, nodes: ids([], ["a"]), edges: ids(["x"]) });
    expect(r.since("demo", E1, { epoch: E1, rev: 3 })).toEqual({ kind: "changed", from: 3, to: 5, nodes: ids(["a"]), edges: ids(["x"]) });
  });

  it("tells a tab to reload when it is older than what is kept, ahead, from another epoch, or past the cap merged", () => {
    const r = followed();
    expect(r.since("demo", E1, { epoch: E1, rev: 2 })).toEqual({ kind: "overflow", rev: 3 });
    expect(r.since("demo", E1, { epoch: E1, rev: 9 })).toEqual({ kind: "overflow", rev: 3 });
    expect(r.since("demo", E1, { epoch: E2, rev: 3 })).toEqual({ kind: "overflow", rev: 3 });
    expect(r.since("demo", E1, { epoch: null, rev: 0 })).toEqual({ kind: "overflow", rev: 3 });
    const half = (p: string) => Array.from({ length: MAX_CHANGE_IDS / 2 + 1 }, (_v, i) => `${p}${i}`);
    r.record(step(3, 4, ids(half("a"))));
    r.record(step(4, 5, ids(half("b"))));
    expect(r.since("demo", E1, { epoch: E1, rev: 4 })).toMatchObject({ kind: "changed", from: 4, to: 5 });
    expect(r.since("demo", E1, { epoch: E1, rev: 3 })).toEqual({ kind: "overflow", rev: 5 });
  });

  it("keeps the last 64 changes, and a tab further behind reloads", () => {
    const r = followed();
    for (let i = 3; i < 3 + 70; i++) r.record(step(i, i + 1, ids([`n${i}`])));
    expect(r.since("demo", E1, { epoch: E1, rev: 8 })).toEqual({ kind: "overflow", rev: 73 });
    expect(r.since("demo", E1, { epoch: E1, rev: 9 })).toMatchObject({ kind: "changed", from: 9, to: 73 });
  });

  it("records a duplicate or older answer once, and a gap as an overflow: never a skipped revision", () => {
    const r = followed();
    r.record(step(3, 4, ids(["a"])));
    r.record(step(3, 4, ids(["a"])));
    r.record(step(2, 3, ids(["z"])));
    expect(r.since("demo", E1, { epoch: E1, rev: 3 })).toEqual({ kind: "changed", from: 3, to: 4, nodes: ids(["a"]), edges: NONE });
    r.record(step(6, 7, ids(["b"])));
    expect(r.rev("demo")).toBe(7);
    expect(r.since("demo", E1, { epoch: E1, rev: 4 })).toEqual({ kind: "overflow", rev: 7 });
  });

  it("forgets a graph no tab follows, and ignores changes for a graph it does not follow", () => {
    const r = followed();
    r.keep(new Set());
    expect(r.has("demo")).toBe(false);
    r.record(overflow(9));
    expect(r.since("demo", E1, { epoch: E1, rev: 0 })).toBeNull();
  });
});

describe("a tab applies each revision once, in order", () => {
  const at = { epoch: E1, rev: 4 };
  const changed = (from: number, to: number) => ({ kind: "changed" as const, from, to, nodes: NONE, edges: NONE });

  it("applies a change that starts where it is", () => {
    expect(plan(at, E1, changed(4, 6))).toBe("apply");
  });

  it("skips a duplicate or an out-of-order answer, and never applies over a gap", () => {
    expect(plan(at, E1, changed(2, 4))).toBe("skip");
    expect(plan(at, E1, changed(3, 6))).toBe("skip");
    expect(plan(at, E1, changed(5, 6))).toBe("skip");
    expect(plan(at, E1, changed(4, 4))).toBe("skip");
  });

  it("reloads on an overflow unless it is already at that revision, and on another epoch", () => {
    expect(plan(at, E1, { kind: "overflow", rev: 4 })).toBe("skip");
    expect(plan(at, E1, { kind: "overflow", rev: 9 })).toBe("reload");
    expect(plan(at, E1, { kind: "overflow", rev: 2 })).toBe("reload");
    expect(plan(at, E2, { kind: "overflow", rev: 4 })).toBe("reload");
    expect(plan(at, E2, changed(4, 6))).toBe("reload");
    expect(plan({ epoch: null, rev: 0 }, E1, { kind: "overflow", rev: 3 })).toBe("reload");
  });
});

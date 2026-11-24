// The spine view (Jordan's brief part 2): the view lays a graph out by umx's placements (`node list --columns
// spine:<name>`), earliest band on the left, and reads a graph's spines and placements through the two read verbs.
import { describe, expect, it, vi } from "vitest";
import { fanOf, nodeOnSpines, type Place, shownSpine, spineLayout, type SpineRead, spineReadsDue, SPREAD, stack, toggleSpine } from "@/lib/graph/spine-layout";
import { listSpines, loadSpinePlaces } from "@/lib/spine";
import { createUmx, UMX_PROTOCOL, type Exec } from "@/lib/umx";

const place = (band: number | null, by: string, anchor = false): Place => ({ band, by, anchor });

describe("laying a graph out along a spine", () => {
  // Two anchors (bands 0 and 1, given out of order), a node in each band, one by rule, one before every anchor, one
  // the spine does not place. Force positions are deliberately scrambled.
  const nodes = [
    { key: "g/a1", id: "a1", x: 0, y: 0 },
    { key: "g/a0", id: "a0", x: 10, y: 5 },
    { key: "g/p0", id: "p0", x: 9, y: 2 },
    { key: "g/p1", id: "p1", x: 1, y: 8 },
    { key: "g/r1", id: "r1", x: 4, y: -3 },
    { key: "g/early", id: "early", x: 6, y: 1 },
    { key: "g/loose", id: "loose", x: 5, y: 4 },
  ];
  const places = new Map<string, Place>([
    ["a0", place(0, "field", true)],
    ["a1", place(1, "field", true)],
    ["p0", place(0, "link")],
    ["p1", place(1, "link")],
    ["r1", place(1, "rule")],
    ["early", place(-1, "field")],
    ["loose", place(null, "not_placed")],
  ]);
  const { at, axis } = spineLayout("g", nodes, places);
  const x = (k: string) => at.get(`g/${k}`)!.x;
  const y = (k: string) => at.get(`g/${k}`)!.y;

  it("puts the bands in the spine's order, earliest on the left, the anchors on one centre line", () => {
    expect(x("early")).toBeLessThan(x("a0"));
    expect(x("a0")).toBeLessThan(x("a1"));
    expect(y("a0")).toBe(y("a1"));
    expect(axis).toMatchObject({ graph: "g", y: y("a0") });
    expect(axis!.bands).toEqual([...axis!.bands].sort((a, b) => a - b));
    expect(axis!.bands).toHaveLength(3);
  });

  it("keeps each placed node in its band's column, a rule placement too, and shelves what is not placed below", () => {
    const half = (axis!.bands[1] - axis!.bands[0]) / 2;
    for (const [k, anchor] of [["p0", "a0"], ["p1", "a1"], ["r1", "a1"]]) expect(Math.abs(x(k) - x(anchor))).toBeLessThan(half);
    expect(y("p1")).toBe(8);
    expect(y("loose")).toBeLessThan(Math.min(...nodes.map((n) => n.y)));
    expect(at.size).toBe(nodes.length);
  });

  it("shows the person's pick, else the main spine, else the first; none when picked none", () => {
    const spines = [{ name: "days", main: false }, { name: "weeks", main: true }];
    expect(shownSpine(spines, undefined)).toBe("weeks");
    expect(shownSpine(spines, "days")).toBe("days");
    expect(shownSpine(spines, null)).toBeNull();
    expect(shownSpine(spines, "gone")).toBe("weeks");
    expect(shownSpine([{ name: "a", main: false }], undefined)).toBe("a");
    expect(shownSpine([], undefined)).toBeNull();
  });
});

describe("how a node sits on each of its graph's spines (node panel)", () => {
  const spines = [
    { name: "days", title: "Days", clock: "calendar" as const, main: true },
    { name: "steps", title: "Steps", clock: "order" as const, main: false },
    { name: "phases", title: "Phases", clock: "order" as const, main: false },
  ];
  const days = new Map<string, Place>([
    ["d0", place(0, "field", true)],
    ["d1", place(1, "field", true)],
    ["n", place(1, "link")],
    ["early", place(-1, "field")],
    ["loose", place(null, "not_placed")],
  ]);
  const steps = new Map<string, Place>([
    ["s0", place(0, "field", true)],
    ["n", place(0, "rule")],
    ["late", place(5, "evidence")],
  ]);
  const reads = (name: string) => (name === "days" ? { state: "ready" as const, places: days } : name === "steps" ? { state: "ready" as const, places: steps } : { state: "loading" as const, places: null });

  it("names the anchor of the node's band and how umx placed it, on every spine, in the graph's order", () => {
    const rows = nodeOnSpines(spines, "n", "days", reads);
    expect(rows.map((r) => [r.name, r.shown, r.state, r.place?.by ?? null, r.anchors])).toEqual([
      ["days", true, "ready", "link", ["d1"]],
      ["steps", false, "ready", "rule", ["s0"]],
      ["phases", false, "loading", null, []],
    ]);
  });

  it("says when a node is not on a spine, is an anchor, or sits outside every anchor's band", () => {
    const [d] = nodeOnSpines(spines.slice(0, 1), "loose", null, reads);
    expect([d.place, d.shown]).toEqual([null, false]);
    expect(nodeOnSpines(spines.slice(0, 1), "d1", null, reads)[0]).toMatchObject({ place: { anchor: true }, anchors: [] });
    expect(nodeOnSpines(spines.slice(0, 1), "early", null, reads)[0]).toMatchObject({ anchors: [], side: "before" });
    expect(nodeOnSpines(spines.slice(1, 2), "late", null, reads)[0]).toMatchObject({ anchors: [], side: "after" });
    expect(nodeOnSpines(spines.slice(0, 1), "elsewhere", null, reads)[0].place).toBeNull();
  });

  it("one toggle for the sidebar and the panel: another spine switches, the shown one goes to none and stays none", () => {
    const list = [{ name: "days", main: true }, { name: "steps", main: false }];
    let pick: string | null | undefined;
    expect(shownSpine(list, pick)).toBe("days");
    pick = toggleSpine(shownSpine(list, pick), "steps");
    expect(shownSpine(list, pick)).toBe("steps");
    pick = toggleSpine(shownSpine(list, pick), "steps");
    expect(pick).toBeNull();
    expect(shownSpine(list, pick)).toBeNull();
    pick = toggleSpine(shownSpine(list, pick), "days");
    expect(shownSpine(list, pick)).toBe("days");
  });
});

describe("when a spine is read again", () => {
  const read = (at: number, loading = false): SpineRead => ({ at, loading, places: new Map(), supported: true });
  const wanted = [["a", "days"], ["a", "steps"], ["b", "days"]] as const;

  it("reads each wanted spine once, and again only after a live change to its own graph", () => {
    const reads = new Map([["a/days", read(0)], ["a/steps", read(0)], ["b/days", read(0)]]);
    expect(spineReadsDue(wanted, reads, new Map())).toEqual([]);
    expect(spineReadsDue(wanted, reads, new Map([["b", 1]]))).toEqual([["b", "days"]]);
    expect(spineReadsDue(wanted, new Map([["a/days", read(0)]]), new Map())).toEqual([["a", "steps"], ["b", "days"]]);
  });

  it("never starts a second read of a spine already being read", () => {
    expect(spineReadsDue([["a", "days"]], new Map([["a/days", read(0, true)]]), new Map([["a", 2]]))).toEqual([]);
  });
});

describe("spreading a spine out", () => {
  // A pile: 30 nodes in band 0 and 12 in band 1, all at nearly the same force position, and 3 the spine does not place.
  const nodes = [
    { key: "g/a0", id: "a0", x: 0, y: 0 },
    { key: "g/a1", id: "a1", x: 1, y: 0 },
    ...Array.from({ length: 42 }, (_, i) => ({ key: `g/n${i}`, id: `n${i}`, x: (i % 3) * 0.01, y: (i % 5) * 0.01 })),
    ...Array.from({ length: 3 }, (_, i) => ({ key: `g/loose${i}`, id: `loose${i}`, x: i, y: 50 })),
    { key: "g/top", id: "top", x: 0, y: 100 },
  ];
  const places = new Map<string, Place>([
    ["a0", place(0, "field", true)],
    ["a1", place(1, "field", true)],
    ...Array.from({ length: 42 }, (_, i): [string, Place] => [`n${i}`, place(i < 30 ? 0 : 1, "link")]),
    ...Array.from({ length: 3 }, (_, i): [string, Place] => [`loose${i}`, place(null, "not_placed")]),
    ["top", place(1, "link")],
  ]);
  const minGap = (at: Map<string, { x: number; y: number }>) => {
    const pts = [...at.entries()].filter(([k]) => !k.startsWith("g/loose")).map(([, p]) => p);
    let least = Infinity;
    for (let i = 0; i < pts.length; i++) for (let j = i + 1; j < pts.length; j++) least = Math.min(least, Math.hypot(pts[i].x - pts[j].x, pts[i].y - pts[j].y));
    return least;
  };
  const gap = 100 / 24;

  it("never sets two nodes of a band on top of each other, or on the anchor, at the default spread", () => {
    const { at } = spineLayout("g", nodes, places);
    expect(minGap(at)).toBeGreaterThanOrEqual(gap - 1e-9);
    expect(at.size).toBe(nodes.length);
  });

  it("fans a band too full for one column, and keeps every node on its own side of the line", () => {
    const { at, axis } = spineLayout("g", nodes, places);
    const xs = new Set(Array.from({ length: 30 }, (_, i) => at.get(`g/n${i}`)!.x));
    expect(xs.size).toBeGreaterThan(1);
    const half = (axis!.bands[1] - axis!.bands[0]) / 2;
    for (const x of xs) expect(Math.abs(x - axis!.bands[0])).toBeLessThan(half);
    expect(at.get("g/top")!.y).toBe(100);
  });

  it("widens the bands and the gaps as the spread grows, and keeps the shelf below everything placed", () => {
    const narrow = spineLayout("g", nodes, places, 1);
    const wide = spineLayout("g", nodes, places, 3);
    expect(wide.axis!.bands[1] - wide.axis!.bands[0]).toBeGreaterThan(1.5 * (narrow.axis!.bands[1] - narrow.axis!.bands[0]));
    expect(minGap(wide.at)).toBeGreaterThanOrEqual(Math.sqrt(3) * gap - 1e-9);
    expect(wide.at.get("g/top")!.y - wide.axis!.y).toBeCloseTo(3 * (100 - narrow.axis!.y));
    for (const { at } of [narrow, wide]) {
      const placed = Math.min(...[...at.entries()].filter(([k]) => !k.startsWith("g/loose")).map(([, p]) => p.y));
      for (let i = 0; i < 3; i++) expect(at.get(`g/loose${i}`)!.y).toBeLessThan(placed);
    }
    expect(spineLayout("g", nodes, places, 99).axis).toEqual(spineLayout("g", nodes, places, SPREAD.default).axis);
  });

  it("fans a band wider as it fills and as the spread grows, never onto the anchor's own column", () => {
    expect([fanOf(0, 1), fanOf(1, 1)]).toEqual([0, 1]);
    for (const n of [2, 5, 15, 30]) for (const s of [0.5, 1, 4]) expect(fanOf(n, s) % 2).toBe(0);
    expect(fanOf(15, 3)).toBeGreaterThan(fanOf(15, 1));
    expect(fanOf(30, 1)).toBeGreaterThan(fanOf(5, 1));
  });

  it("stacks one side of the line off it, each where the force had it when there is room", () => {
    expect(stack([1, 1, 1, 10], 0, 2, 1)).toEqual([2, 4, 6, 10]);
    expect(stack([-1, -9], 0, 2, -1)).toEqual([-2, -9]);
  });
});

describe("reading spines", () => {
  const env = (command: string, data: unknown) => JSON.stringify({ umx: UMX_PROTOCOL, command, status: "ok", data, warnings: [], errors: [] });
  const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA";
  const N = "01NODEAAAAAAAAAAAAAAAAAAAA";
  const M = "01NODEBBBBBBBBBBBBBBBBBBBB";
  function harness(rows: unknown[]) {
    const exec = vi.fn<Exec>(async (_bin, argv) => {
      const verb = `${argv[0]} ${argv[1]}`;
      if (verb === "graph list") return { stdout: env(verb, { rows: [{ alias: "demo", id: GA, path: "/h/g/demo", state: "loaded", folder: "present", policy: "open" }] }), failed: false, detail: "" };
      if (verb === "graph spine") {
        const decl = (name: string, main: boolean) => ({ name, title: `${name} title`, main, anchors: { type: "day", inside: null }, order: ["starts_on"], place: ["raised_in"] });
        return { stdout: env("graph spine show", { spines: [{ decl: decl("weeks", false), clock: "order", cover: { total: 3 } }, { decl: decl("days", true), clock: "calendar", cover: { total: 3, not_placed: 1 } }, { decl: { name: "Bad Name" } }] }), failed: false, detail: "" };
      }
      if (verb === "node list") return { stdout: env(verb, { rows, total: rows.length, truncated: false }), failed: false, detail: "" };
      return { stdout: env(verb, {}), failed: false, detail: "" };
    });
    return createUmx({ exec, bin: () => "/u", env: () => ({}) });
  }

  it("lists the declared spines, the main one first, and skips a name umx would not write", async () => {
    const spines = await listSpines(harness([]), "demo");
    expect(spines.map((s) => [s.name, s.main, s.clock])).toEqual([["days", true, "calendar"], ["weeks", false, "order"]]);
    expect(spines[0].cover).toMatchObject({ total: 3, notPlaced: 1 });
  });

  it("reads each node's band and how, and says when this umx gives no placements", async () => {
    const got = await loadSpinePlaces(harness([{ id: N, "spine:days": { band: 2, by: "link", anchor: false } }, { id: M, "spine:days": { band: null, by: "odd", anchor: true } }]), "demo", "days");
    expect(got).toEqual({ graph: "demo", spine: "days", places: [[N, 2, "link", 0], [M, null, "not_placed", 1]], supported: true, gone: false });
    const old = await loadSpinePlaces(harness([{ id: N }]), "demo", "days");
    expect(old.supported).toBe(false);
    // The column is there but empty: the spine was removed since it was listed, not an older umx.
    const gone = await loadSpinePlaces(harness([{ id: N, "spine:days": null }]), "demo", "days");
    expect([gone.supported, gone.gone]).toEqual([true, true]);
  });
});

// K1 FUI skins (plan 130, ui-expert design note): the shipped token contrast per brightness level, the type hue
// across the four themes, the N7 hue budget, the N11 relation palette, callout text, leaders and glow.
import { describe, expect, it } from "vitest";
import { themeChoice } from "@/components/theme";
import {
  assignCustomTypes,
  assignRelations,
  CUSTOM_HUES,
  ground,
  HUE_GAP,
  hueDistance,
  isRed,
  RELATION_HUES,
  relationColor,
  skinOf,
  THEMES,
  themeOf,
  typeColor,
  typeHue,
} from "@/lib/graph/colors";
import { activeNotes, NO_AGENT, reduce, qualifyEvent } from "@/lib/agent";
import { qid } from "@/lib/graph/keys";
import {
  calloutIn,
  calloutText,
  COLUMN_INSET,
  columnWidth,
  effectiveFocus,
  fitCallout,
  type Frame,
  GLOW_CAP,
  glowSet,
  LEADER_CAP,
  type LeaderCandidate,
  liveGlow,
  MIN_COLUMN,
  leaderChoice,
  placeLeaders,
  leaderEnd,
  moreCallouts,
  glowRadii,
  pillLook,
  FUI_PILLS,
} from "@/lib/graph/fui";
import type { UiCommand } from "@/lib/umx/socket";

// The shipped stylesheet's own text (Vite's ?raw), read the way the browser gets it, not through the app's code.
import CSS from "../app/globals.css?raw";

/** The custom properties a top-level `selector { ... }` block sets in globals.css. */
function tokens(selector: string): Record<string, string> {
  const m = new RegExp(`(?:^|\\n)${selector.replace(".", "\\.")} \\{([^}]*)\\}`).exec(CSS);
  if (!m) throw new Error(`no ${selector} block`);
  return Object.fromEntries([...m[1].matchAll(/(--[a-z-]+):\s*([^;]+);/g)].map((t) => [t[1], t[2].trim()]));
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** #rrggbb to OKLCH [l, c, h]: a second path back from the hex the app ships, not the one that made it. */
function oklch(hex: string): [number, number, number] {
  const lin = [1, 3, 5].map((i) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  const [r, g, b] = lin;
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [L, Math.hypot(A, B), ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360];
}

const STD = ["person", "organisation", "team", "role", "project", "meeting", "event", "action", "question", "topic",
  "risk", "decision", "document", "transcript", "place", "system"];
const SLATE = ["file", "url", "folder", "excerpt"];

describe("FUI token contrast, per brightness level (design note proof 2)", () => {
  for (const theme of ["console", "instrument"] as const) {
    const t = tokens(`.${theme}`);
    const bg = t["--background"];
    const c = (name: string) => contrast(t[name], bg);
    it(`${theme}: data >= 7, structure and chrome text >= 4.5, chrome lines 1.3 to 2.5, accent and agent >= 4.5`, () => {
      for (const data of ["--foreground", "--canvas-ink", "--readout"]) expect(c(data), data).toBeGreaterThanOrEqual(7);
      expect(c("--muted-foreground")).toBeGreaterThanOrEqual(4.5);
      expect(c("--chrome-text")).toBeGreaterThanOrEqual(4.5);
      for (const line of ["--border", "--chrome"]) {
        expect(c(line), line).toBeGreaterThanOrEqual(1.3);
        expect(c(line), line).toBeLessThanOrEqual(2.5);
      }
      for (const name of ["--primary", "--ring", "--glow-select", "--canvas-agent", "--glow-agent"]) expect(c(name), name).toBeGreaterThanOrEqual(4.5);
      // Three levels: data brightest, then structure, then chrome (readability rule 1).
      expect(c("--foreground")).toBeGreaterThan(c("--muted-foreground"));
      expect(c("--muted-foreground")).toBeGreaterThan(c("--chrome"));
      expect(t["--canvas"]).toBe(bg);
      expect(t["--radius"]).toBe("2px");
    });
  }

  it("ships no remote asset: no url() anywhere in the styles (security K1)", () => {
    expect(CSS).toContain("@custom-variant fui");
    expect(CSS).not.toMatch(/url\s*\(/i);
  });
});

describe("themes", () => {
  it("has four themes on two grounds, and only Console and Instrument are FUI", () => {
    expect(THEMES).toEqual(["light", "dark", "console", "instrument"]);
    expect(THEMES.map(ground)).toEqual(["light", "dark", "dark", "light"]);
    expect(THEMES.map(skinOf)).toEqual(["classic", "classic", "fui", "fui"]);
  });

  it("persists and reads a theme as one of the fixed names only (security K1-c)", () => {
    for (const name of ["light", "dark", "console", "instrument", "system"]) expect(themeChoice(name)).toBe(name);
    for (const bad of ["<script>", "Console", "", null, undefined, 3, { theme: "console" }]) expect(themeChoice(bad)).toBe("system");
    expect(themeOf("instrument")).toBe("instrument");
    expect(themeOf("system")).toBe("light");
    expect(themeOf(undefined)).toBe("light");
  });

  it("keeps each type's hue in all four themes; only lightness changes (rule 4, proof 5)", () => {
    const custom = ["problem", "option", "criterion", "capability", "constraint", "goal"];
    assignCustomTypes([...STD, ...custom]);
    for (const type of [...STD, ...custom]) {
      const looks = THEMES.map((theme) => oklch(typeColor(type, theme)));
      for (const [, c, h] of looks) {
        expect(c).toBeGreaterThan(0.05);
        expect(hueDistance(h, typeHue(type)), `${type}`).toBeLessThan(4);
      }
      // Data stays the brightest level: Console lighter than Dark, Instrument darker than Light.
      const [light, dark, console, instrument] = looks.map((l) => l[0]);
      expect(console).toBeGreaterThan(dark);
      expect(instrument).toBeLessThan(light);
    }
    for (const type of SLATE) for (const theme of THEMES) expect(typeColor(type, theme)).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("N7: the custom hue budget", () => {
  it("has 8 hues, each 12 degrees or more from every std hue and from each other, none in 345 to 30", () => {
    expect(HUE_GAP).toBe(12);
    expect(CUSTOM_HUES).toEqual([92, 104, 116, 128, 177, 302, 330, 342]);
    for (const h of CUSTOM_HUES) {
      expect(isRed(h)).toBe(false);
      for (const s of [...STD, ...SLATE]) expect(hueDistance(h, typeHue(s))).toBeGreaterThanOrEqual(HUE_GAP);
      for (const o of CUSTOM_HUES) if (o !== h) expect(hueDistance(h, o)).toBeGreaterThanOrEqual(HUE_GAP);
    }
  });

  it("never gives a custom type a red, assigned or by hash", () => {
    const names = Array.from({ length: 300 }, (_, i) => `kind_${i}_${(i * 104729).toString(36)}`);
    assignCustomTypes([]);
    for (const n of names) expect(isRed(typeHue(n))).toBe(false);
    assignCustomTypes(names);
    for (const n of names) expect(isRed(typeHue(n))).toBe(false);
    expect(isRed(typeHue("risk"))).toBe(true);
  });
});

describe("N11: the relation palette", () => {
  const relations = ["supports", "blocks", "depends_on", "mentions", "decided_in", "owns", "raised_by", "answers", "contradicts", "follows"];

  it("gives the relations of one graph their own low-chroma slots, by probing in name order", () => {
    assignRelations(relations);
    for (const theme of THEMES) {
      const colours = relations.map((r) => relationColor(r, theme));
      expect(new Set(colours).size).toBe(relations.length);
      for (const colour of colours) {
        const [, c, h] = oklch(colour);
        expect(c).toBeLessThan(0.075);
        // Its hue is one of the palette's, so never a red.
        expect(Math.min(...RELATION_HUES.map((p) => hueDistance(p, h)))).toBeLessThan(4);
      }
    }
    expect(RELATION_HUES).toHaveLength(10);
    for (const h of RELATION_HUES) expect(isRed(h)).toBe(false);
  });

  it("never reads as a type fill: every relation is clearly less saturated than every type", () => {
    assignRelations(relations);
    assignCustomTypes(["problem", "option"]);
    for (const theme of THEMES) {
      const most = Math.max(...relations.map((r) => oklch(relationColor(r, theme))[1]));
      const least = Math.min(...[...STD, "problem", "option"].map((t) => oklch(typeColor(t, theme))[1]));
      expect(most + 0.02).toBeLessThan(least);
    }
  });
});

describe("callout text (security K1-b)", () => {
  const FSI = "\u2068";
  const PDI = "\u2069";
  const segments = (s: string) => [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(s)].map((g) => g.segment);

  it("drops controls and bidi overrides, and isolates the label once", () => {
    const out = calloutText("Pay \u202Eredaolnwod\u202C now\n\u0000\u2066x\u2069\u200f\u0085done");
    expect(out.startsWith(FSI) && out.endsWith(PDI)).toBe(true);
    const inner = out.slice(1, -1);
    expect(inner).toBe("Pay redaolnwod now x done");
    const bad = (ch: number) => ch < 0x20 || (ch >= 0x7f && ch <= 0x9f) || ch === 0x061c || ch === 0x200e || ch === 0x200f || (ch >= 0x202a && ch <= 0x202e) || (ch >= 0x2066 && ch <= 0x2069);
    expect([...inner].some((ch) => bad(ch.charCodeAt(0)))).toBe(false);
  });

  it("drops zero-width characters with no space (security N1)", () => {
    const out = calloutText("pay\u200Bpal \u200Cac\u200Dcount\uFEFF ok\u200B");
    expect(out).toBe(`${FSI}paypal account ok${PDI}`);
    expect([...out].some((ch) => /[\u200b-\u200d\ufeff]/.test(ch))).toBe(false);
  });

  it("caps by grapheme, so a flag, a skin-toned emoji, an accent or an Arabic word is never split", () => {
    for (const g of ["🇦🇺", "👍🏽"]) {
      const inner = segments(calloutText(g.repeat(60)).slice(1, -1));
      expect(inner).toHaveLength(40);
      expect(inner.slice(0, 39).every((x) => x === g)).toBe(true);
      expect(inner[39]).toBe("…");
    }
    const accents = calloutText("e\u0301".repeat(50));
    expect(segments(accents.slice(1, -1)).slice(0, 39).every((g) => g === "e\u0301")).toBe(true);
    const arabic = calloutText(`${"مرحبا ".repeat(20)}\u202E`);
    expect(arabic.endsWith(PDI)).toBe(true);
    expect(arabic.split(PDI)).toHaveLength(2);
    expect(calloutText("short")).toBe(`${FSI}short${PDI}`);
  });
});

describe("leaders (rule 5)", () => {
  const frame: Frame = { width: 1000, height: 700, left: { edge: 0, top: 36, bottom: 76 }, right: { edge: 1000, top: 220, bottom: 132 }, gap: 6 };
  const c = (id: string, x: number, y: number, kind: LeaderCandidate["kind"] = "select"): LeaderCandidate => ({ id, kind, x, y, h: 46, w: 120, r: 4 });

  it("places at most 12, selection first, and counts the rest", () => {
    const cands = [...Array.from({ length: 5 }, (_, i) => c(`s${i}`, 100 + i * 150, 100 + i * 90)), ...Array.from({ length: 15 }, (_, i) => c(`a${i}`, 50 + i * 60, 80 + i * 35, "agent"))];
    const { placed, more } = placeLeaders(cands, frame);
    expect(placed).toHaveLength(LEADER_CAP);
    expect(more).toBe(cands.length - LEADER_CAP);
    for (let i = 0; i < 5; i++) expect(placed.some((p) => p.id === `s${i}`)).toBe(true);
  });

  it("measures only the callouts that can be placed: the first of each id inside the frame, at most 12 (reviewer N1)", () => {
    const cands = [c("off", -20, 100), c("s0", 300, 300), c("s0", 300, 300, "agent"), ...Array.from({ length: 30 }, (_, i) => c(`a${i}`, 50 + i * 30, 80 + i * 20, "agent"))];
    const { chosen, beyond } = leaderChoice(cands, frame);
    expect(chosen.map((x) => x.id)).toEqual(["s0", ...Array.from({ length: 11 }, (_, i) => `a${i}`)]);
    expect(beyond).toBe(31 - LEADER_CAP);
    // Placing the chosen ones gives what placing them all gave, the rest counted in `more`.
    const all = placeLeaders(cands, frame);
    const some = placeLeaders(chosen, frame);
    expect([some.placed, some.more + beyond]).toEqual([all.placed, all.more]);
  });

  it("puts each callout in the nearest column with room, inside the frame, never overlapping", () => {
    const cands = Array.from({ length: 12 }, (_, i) => c(`n${i}`, i % 2 ? 800 : 200, 300 + (i % 3)));
    const { placed, more } = placeLeaders(cands, frame);
    expect(placed.length + more).toBe(12);
    for (const side of ["left", "right"] as const) {
      const col = placed.filter((p) => p.side === side).sort((a, b) => a.top - b.top);
      for (const p of col) {
        expect(p.top).toBeGreaterThanOrEqual(frame[side].top);
        expect(p.top + p.h).toBeLessThanOrEqual(frame.height - frame[side].bottom);
      }
      for (let i = 1; i < col.length; i++) expect(col[i].top).toBeGreaterThanOrEqual(col[i - 1].top + col[i - 1].h + frame.gap);
    }
    expect(placed.find((p) => p.id === "n0")?.side).toBe("left");
    expect(placed.find((p) => p.id === "n1")?.side).toBe("right");
  });

  it("never covers its own node: the other column, else past the node, else left out (ui-expert K1-N1)", () => {
    const covered = (p: { side: "left" | "right"; top: number; h: number; w?: number }, d: { x: number; y: number; r: number }) => {
      const x = p.side === "left" ? frame.left.edge + 16 : frame.right.edge - 16 - (p.w ?? 0);
      const nx = Math.min(Math.max(d.x, x), x + (p.w ?? 0));
      const ny = Math.min(Math.max(d.y, p.top), p.top + p.h);
      return Math.hypot(d.x - nx, d.y - ny) < d.r + 4;
    };
    // At x 60 the left column's box would sit on the node: it goes right.
    const flipped = placeLeaders([c("edge", 60, 300)], frame).placed[0];
    expect(flipped.side).toBe("right");
    // On a canvas so narrow both columns cover x 200, the box stays left and moves past its node.
    const narrow: Frame = { ...frame, width: 420, right: { ...frame.right, edge: 420 } };
    const wide = { ...c("mid", 200, 300), w: 170 };
    const moved = placeLeaders([wide], narrow).placed[0];
    expect(moved.side).toBe("left");
    expect(covered(moved, { x: 200, y: 300, r: 4 })).toBe(false);
    // A column full of nodes under the band: every placed box keeps clear of its node and of the others.
    const busy = Array.from({ length: 8 }, (_, i) => ({ ...c(`b${i}`, 200, 120 + i * 50), w: 170 }));
    const { placed, more } = placeLeaders(busy, narrow);
    expect(placed.length + more).toBe(8);
    for (const p of placed) expect(covered(p, { x: p.x, y: p.y, r: 4 })).toBe(false);
    const col = [...placed].sort((a, b) => a.top - b.top);
    for (let i = 1; i < col.length; i++) expect(col[i].top).toBeGreaterThanOrEqual(col[i - 1].top + col[i - 1].h + frame.gap);
  });

  it("never covers the selected node, whichever callout it is", () => {
    const selected = { x: 940, y: 400, r: 6 };
    const { placed } = placeLeaders([c("agent", 800, 400, "agent")], { ...frame, keepClear: selected });
    const p = placed[0];
    const x = frame.right.edge - 16 - 120;
    const nx = Math.min(Math.max(selected.x, x), x + 120);
    const ny = Math.min(Math.max(selected.y, p.top), p.top + p.h);
    expect(Math.hypot(selected.x - nx, selected.y - ny)).toBeGreaterThanOrEqual(selected.r + 4);
    // Without it, the box sits level with its node, over the selected one.
    const plain = placeLeaders([c("agent", 800, 400, "agent")], frame).placed[0];
    expect(plain.top).toBe(400 - 23);
  });

  it("ends a leader at the box point nearest its node, and labels the callouts left out", () => {
    expect(leaderEnd({ x: 500, y: 300 }, { x: 16, y: 280, w: 120, h: 46 })).toEqual({ x: 136, y: 300 });
    expect(leaderEnd({ x: 500, y: 100 }, { x: 16, y: 280, w: 120, h: 46 })).toEqual({ x: 136, y: 280 });
    expect(moreCallouts(285)).toBe("CALLOUTS +285");
  });

  it("draws the FUI skins' cluster pills quietly and at most 12, the others as before (ui-expert K1-N2)", () => {
    const palette = { ink: "#ink", structure: "#structure", chromeHi: "#chromehi" };
    expect(pillLook(true, palette, "#group", 40)).toEqual({ text: "#structure", border: "#chromehi", cap: FUI_PILLS });
    expect(FUI_PILLS).toBe(12);
    expect(pillLook(false, palette, "#group", 40)).toEqual({ text: "#ink", border: "#group", cap: 40 });
  });

  it("starts a node's glow at its edge, not its centre (ui-expert K1-N4)", () => {
    expect(glowRadii(10)).toEqual({ inner: 10, outer: 30 });
    expect(glowRadii(1)).toEqual({ inner: 1, outer: 8 });
  });

  it("gives no callout to a node outside the frame or under the node panel", () => {
    const panel = { ...frame, right: { ...frame.right, edge: 580 } };
    const { placed, more } = placeLeaders([c("off", -20, 100), c("under", 700, 300), c("in", 300, 300), c("in", 300, 300)], panel);
    expect(placed.map((p) => p.id)).toEqual(["in"]);
    expect(more).toBe(0);
  });
});

describe("glow (rule 2)", () => {
  const all = () => true;

  it("glows only the selection, the agent's nodes and a live change, each once, in that order", () => {
    const { glow, wanted } = glowSet(["a", "b"], ["b", "c"], ["c", "d"], all);
    expect(glow).toEqual([["a", "select"], ["b", "select"], ["c", "agent"], ["d", "live"]]);
    expect(wanted).toBe(4);
    expect(glowSet([], [], [], all).glow).toEqual([]);
    expect(glowSet(["hidden"], ["hidden"], ["hidden"], (id) => id !== "hidden").glow).toEqual([]);
  });

  it("caps at 200, keeping the selection and the first agent nodes", () => {
    const sel = Array.from({ length: 150 }, (_, i) => `s${i}`);
    const agent = Array.from({ length: 100 }, (_, i) => `a${i}`);
    const { glow, wanted } = glowSet(sel, agent, [], all);
    expect(glow).toHaveLength(GLOW_CAP);
    expect(wanted).toBe(250);
    expect(glow.filter(([, k]) => k === "select")).toHaveLength(150);
    expect(glow.at(-1)).toEqual(["a49", "agent"]);
  });

  it("fades a live change in 600 ms, holds 1.2 s and fades out 600 ms, once; reduced motion is on and still", () => {
    expect(liveGlow(0, false)).toBe(0);
    expect(liveGlow(300, false)).toBeCloseTo(0.5);
    expect(liveGlow(600, false)).toBe(1);
    expect(liveGlow(1799, false)).toBe(1);
    expect(liveGlow(2100, false)).toBeCloseTo(0.5);
    expect(liveGlow(2400, false)).toBe(0);
    expect(liveGlow(10_000, false)).toBe(0);
    expect([0, 300, 2100].map((t) => liveGlow(t, true))).toEqual([1, 1, 1]);
    expect(liveGlow(2400, true)).toBe(0);
    expect(calloutIn(100, false)).toEqual({ leader: 0.5, text: 0 });
    expect(calloutIn(300, false).leader).toBe(1);
    expect(calloutIn(0, true)).toEqual({ leader: 1, text: 1 });
  });
});

describe("callouts fit their box (review B1)", () => {
  const FSI = "\u2068";
  const PDI = "\u2069";
  const seg = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  const parts = (t: string) => [...seg.segment(t)].map((g) => g.segment);
  // A stand-in for measureText: wide capitals, CJK and emoji are wider than the box at 40 graphemes.
  const measure = (t: string) => parts(t).reduce((w, g) => w + (g === FSI || g === PDI ? 0 : /[\u3000-\u9fff]/.test(g) ? 13 : /\p{Extended_Pictographic}/u.test(g) ? 16 : g === "W" ? 11 : 6), 0);

  for (const [name, text, unit] of [["wide Latin", "W".repeat(60), "W"], ["CJK", "漢字".repeat(30), null], ["emoji", "🇦🇺".repeat(60), "🇦🇺"]] as const) {
    it(`cuts ${name} at a grapheme to the inner width, with a closed isolate`, () => {
      for (const width of [280, 104, 30]) {
        const out = fitCallout(text, width, measure);
        expect(measure(out)).toBeLessThanOrEqual(width);
        expect(out.startsWith(FSI) && out.endsWith(PDI)).toBe(true);
        expect(out.split(PDI)).toHaveLength(2);
        const inner = parts(out.slice(1, -1));
        expect(inner.at(-1)).toBe("…");
        if (unit) expect(inner.slice(0, -1).every((g) => g === unit)).toBe(true);
        else expect(inner.slice(0, -1).every((g) => g === "漢" || g === "字")).toBe(true);
      }
    });
  }

  it("leaves text that fits as calloutText has it, and never exceeds 40 graphemes", () => {
    expect(fitCallout("short title", 280, measure)).toBe(calloutText("short title"));
    expect(parts(fitCallout("a".repeat(100), 10_000, measure).slice(1, -1))).toHaveLength(40);
    expect(fitCallout("WWWW", 0, measure)).toBe(`${FSI}…${PDI}`);
  });

  it("keeps the two columns apart on a narrow canvas, and draws none when there is no room", () => {
    for (const right of [1000, 700, 400, 260]) {
      const f = { left: { edge: 0, top: 36, bottom: 76 }, right: { edge: right, top: 36, bottom: 76 } };
      const w = columnWidth(f);
      expect(w).toBeLessThanOrEqual(300);
      expect(f.left.edge + COLUMN_INSET + w).toBeLessThanOrEqual(f.right.edge - COLUMN_INSET - w);
    }
    const tiny: Frame = { width: 200, height: 600, left: { edge: 0, top: 36, bottom: 76 }, right: { edge: 200, top: 36, bottom: 76 }, gap: 6 };
    expect(columnWidth(tiny)).toBeLessThan(MIN_COLUMN);
    expect(placeLeaders([{ id: "a", kind: "select", x: 50, y: 300, h: 46 }], tiny)).toEqual({ placed: [], more: 1 });
  });
});

describe("agent notes come from what is active (review B2)", () => {
  const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
  const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
  let seq = 0;
  // Events reach reduce qualified by their graph, as receive gives them (plan 133); notes are keyed the same way.
  const run = (...commands: UiCommand[]) => commands.reduce((s, command) => reduce(s, qualifyEvent({ seq: ++seq, at_ms: 1, graph: "demo", by: null, command })), NO_AGENT);
  const [kA, kB] = [A, B].map((n) => qid("demo", n));

  it("drops a note on clear, on a replacing highlight and on another focus; history keeps the words", () => {
    const cleared = run({ kind: "highlight", nodes: [A], add: false, note: "old" }, { kind: "clear" });
    expect(activeNotes(cleared).size).toBe(0);
    expect(cleared.history.some((e) => e.command.note === "old")).toBe(true);
    expect([...activeNotes(run({ kind: "highlight", nodes: [A], add: false, note: "old" }, { kind: "highlight", nodes: [B], add: false }))]).toEqual([]);
    expect([...activeNotes(run({ kind: "focus", node: A, hops: 1, note: "here" }, { kind: "focus", node: B, hops: 1 }))]).toEqual([]);
    expect([...activeNotes(run({ kind: "focus", node: A, hops: 1, note: "here" }))]).toEqual([[kA, "here"]]);
  });

  it("lets a newer annotation of the same node take its note away, or set its own", () => {
    expect(activeNotes(run({ kind: "highlight", nodes: [A], add: false, note: "old" }, { kind: "highlight", nodes: [A], add: true })).has(kA)).toBe(false);
    expect(activeNotes(run({ kind: "highlight", nodes: [A], add: false, note: "old" }, { kind: "focus", node: A, hops: 1 })).has(kA)).toBe(false);
    expect(activeNotes(run({ kind: "focus", node: A, hops: 1, note: "f" }, { kind: "highlight", nodes: [A, B], add: false, note: "h" })).get(kA)).toBe("h");
    expect(activeNotes(run({ kind: "highlight", nodes: [A, B], add: false, note: "h" }, { kind: "focus", node: A, hops: 2, note: "f" })).get(kA)).toBe("f");
    expect(activeNotes(run({ kind: "highlight", nodes: [A, B], add: false, note: "h" }, { kind: "focus", node: A, hops: 2, note: "f" })).get(kB)).toBe("h");
  });
});

describe("the FOCUS readout reads the focus the canvas draws (review B3)", () => {
  const person = { depths: new Map([["a", 0], ["b", 1]]), hops: 1 };
  const agent = { depths: new Map([["c", 0], ["d", 1], ["e", 2]]), hops: 2 };
  it("is the person's when on, else the agent's, else none", () => {
    expect(effectiveFocus(person, agent)).toBe(person);
    expect(effectiveFocus(null, agent)).toBe(agent);
    expect(effectiveFocus(null, null)).toBeNull();
  });
});

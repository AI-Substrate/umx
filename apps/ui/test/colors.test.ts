import { describe, expect, it } from "vitest";
import {
  assignCustomTypes,
  CUSTOM_HUES,
  HUE_GAP,
  hueDistance,
  isStdType,
  oklchToHex,
  statusLook,
  typeColor,
  typeHue,
} from "@/lib/graph/colors";
import { safeHttpUrl } from "@/lib/safe-url";

describe("colours", () => {
  it("gives a type the same colour in every graph, distinct per theme", () => {
    expect(typeColor("widget_x", "light")).toBe(typeColor("widget_x", "light"));
    expect(typeColor("widget_x", "light")).not.toBe(typeColor("widget_x", "dark"));
    expect(typeColor("risk", "dark")).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("uses the curated family hue for std types", () => {
    expect(typeHue("risk")).toBe(25);
    expect(typeHue("decision")).toBe(318);
  });

  it("stays in sRGB", () => {
    for (let h = 0; h < 360; h += 15) expect(oklchToHex(0.74, 0.3, h)).toMatch(/^#[0-9a-f]{6}$/);
    expect(oklchToHex(1, 0, 0)).toBe("#ffffff");
    expect(oklchToHex(0, 0, 0)).toBe("#000000");
  });

  it("rings blocked red and mutes settled nodes", () => {
    expect(statusLook("blocked", "light").kind).toBe("blocked");
    expect(statusLook("agreed", "light").muted).toBe(true);
    expect(statusLook("something_custom", "dark").kind).toBe("other");
  });
});

const STD = ["person", "organisation", "team", "role", "project", "meeting", "event", "action", "question", "topic",
  "risk", "decision", "document", "transcript", "place", "system", "file", "url", "folder", "excerpt"];

describe("custom type hues (ui-expert N1)", () => {
  it("keeps every custom hue slot HUE_GAP degrees or more from every std hue and from each other", () => {
    expect(STD.every(isStdType)).toBe(true);
    expect(CUSTOM_HUES.length).toBeGreaterThan(0);
    for (const h of CUSTOM_HUES) {
      for (const s of STD) expect(hueDistance(h, typeHue(s))).toBeGreaterThanOrEqual(HUE_GAP);
      for (const o of CUSTOM_HUES) if (o !== h) expect(hueDistance(h, o)).toBeGreaterThanOrEqual(HUE_GAP);
    }
  });

  it("never lands a custom type on a std hue, assigned or not", () => {
    const names = Array.from({ length: 400 }, (_, i) => `custom_${i}_${(i * 7919).toString(36)}`);
    const check = () => {
      for (const n of names) for (const s of STD) expect(hueDistance(typeHue(n), typeHue(s))).toBeGreaterThanOrEqual(HUE_GAP);
    };
    assignCustomTypes([]);
    check();
    assignCustomTypes(names);
    check();
  });

  it("gives the custom types of one graph different colours, by probing in name order", () => {
    // The types earlier screenshots showed clashing with std hues.
    const custom = ["problem", "option", "criterion", "capability", "constraint", "goal"];
    assignCustomTypes([...custom, "project", "decision"]);
    for (const theme of ["light", "dark"] as const) {
      const colours = custom.map((t) => typeColor(t, theme));
      expect(new Set(colours).size).toBe(custom.length);
      for (const s of STD) expect(colours).not.toContain(typeColor(s, theme));
    }
  });
});

/** WCAG relative luminance contrast of two #rrggbb colours. */
function contrast(a: string, b: string): number {
  const lum = (c: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const v = parseInt(c.slice(i, i + 2), 16) / 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

describe("status rings (ui-expert N2)", () => {
  const canvas = { light: "#f7f4ef", dark: "#1f1e1d" } as const;
  for (const theme of ["light", "dark"] as const) {
    it(`settled recedes and blocked stays loud (${theme})`, () => {
      const [open, settled, blocked] = ["open", "done", "blocked"].map((s) => statusLook(s, theme));
      expect(settled.ringSize).toBeLessThan(open.ringSize);
      expect(blocked.ringSize).toBeGreaterThan(open.ringSize);
      expect(contrast(settled.ring, canvas[theme])).toBeLessThan(1.6);
      expect(contrast(settled.ring, canvas[theme])).toBeLessThan(contrast(open.ring, canvas[theme]));
      expect(contrast(blocked.ring, canvas[theme])).toBeGreaterThan(3);
    });
  }
});

describe("links", () => {
  it("links only http(s)", () => {
    expect(safeHttpUrl("https://example.com/a?b")).toBe("https://example.com/a?b");
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "JAVASCRIPT:alert(1)", " javascript:x", "file:///etc/passwd", "calls/3-nov.vtt", "umx:demo/01Z"]) {
      expect(safeHttpUrl(bad)).toBeNull();
    }
  });
});

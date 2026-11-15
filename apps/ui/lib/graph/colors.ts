// Colour from type and status, chosen by the UI (D42, design note section 4). Std v1 types (D57) get a curated
// hue by family; any other type gets a slot from its name, kept clear of the std hues (assignCustomTypes). Hues
// live in OKLCH with fixed lightness and chroma per theme, so every type reads in every theme, and a theme changes
// a type's lightness, never its hue (K1 skins, readability rule 4).

/** The four themes (K1): today's Light and Dark, and the FUI skins Console (dark) and Instrument (light). */
export const THEMES = ["light", "dark", "console", "instrument"] as const;
export type Theme = (typeof THEMES)[number];
/** The ground a theme sits on: Console is dark, Instrument is light. */
export type Ground = "light" | "dark";
export type Skin = "classic" | "fui";

export function ground(theme: Theme): Ground {
  return theme === "dark" || theme === "console" ? "dark" : "light";
}

export function skinOf(theme: Theme): Skin {
  return theme === "console" || theme === "instrument" ? "fui" : "classic";
}

/** The theme a stored or resolved name stands for; anything else (system, unknown, missing) reads as Light. */
export function themeOf(name: string | undefined): Theme {
  return (THEMES as readonly string[]).includes(name ?? "") ? (name as Theme) : "light";
}

type Family = { hue: number; chroma?: number };

const STD: Record<string, Family> = {
  // people: warm oranges
  person: { hue: 55 },
  organisation: { hue: 40 },
  team: { hue: 68 },
  role: { hue: 80 },
  // work: blues and teals, risk red, decision purple
  project: { hue: 250 },
  meeting: { hue: 228 },
  event: { hue: 205 },
  action: { hue: 190 },
  question: { hue: 238 },
  topic: { hue: 215 },
  risk: { hue: 25 },
  decision: { hue: 318 },
  // sources: violet
  document: { hue: 290 },
  transcript: { hue: 275 },
  // places and things: green
  place: { hue: 145 },
  system: { hue: 165 },
  // resources: slate
  file: { hue: 255, chroma: 0.03 },
  url: { hue: 240, chroma: 0.03 },
  folder: { hue: 265, chroma: 0.03 },
  excerpt: { hue: 230, chroma: 0.03 },
};

/** Reserved provenance types (D55); hidden by default. */
export const PROVENANCE_TYPES = new Set(["session", "agent", "harness", "model", "platform"]);
/** Out-of-play statuses (D69): hidden by default, behind the one "out of play" toggle (Visibility.retired). */
export const RETIRED_STATUSES = new Set(["retired", "superseded", "ruled_out"]);

// Console is the dark tone at l 0.78, Instrument the light tone at l 0.58: data stays the brightest level.
const TONE: Record<Theme, { l: number; c: number }> = {
  light: { l: 0.63, c: 0.15 },
  dark: { l: 0.74, c: 0.13 },
  console: { l: 0.78, c: 0.13 },
  instrument: { l: 0.58, c: 0.15 },
};

export function isStdType(type: string): boolean {
  return Object.hasOwn(STD, type);
}

/** FNV-1a, 32 bit. */
export function hash(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** Shortest distance between two hues, in degrees. */
export function hueDistance(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
}

/** How far a custom type's hue stays from every std hue and from the other custom hues (ui-expert N1, N7). */
export const HUE_GAP = 12;
/** Custom hues never fall in RED_FROM..360 or 0..RED_TO: red stays for risk and the blocked ring (ui-expert N7). */
export const RED_FROM = 345;
export const RED_TO = 30;

export function isRed(hue: number): boolean {
  const h = ((hue % 360) + 360) % 360;
  return h >= RED_FROM || h <= RED_TO;
}

/** The hues custom types may take: every hue outside the red band and at least HUE_GAP from the std hues and from
 * each other, picked greedily from 0. The std palette is dense, so there are only a few (8); lightness tiers
 * multiply them. */
export const CUSTOM_HUES: readonly number[] = (() => {
  const taken = Object.values(STD).map((f) => f.hue);
  const out: number[] = [];
  for (let h = 0; h < 360; h++) {
    if (!isRed(h) && [...taken, ...out].every((t) => hueDistance(h, t) >= HUE_GAP)) out.push(h);
  }
  return out;
})();
/** Lightness offsets for custom types once the hues are used up: base, darker, lighter. */
const TIERS = [0, -0.12, 0.1];
const SLOTS = CUSTOM_HUES.length * TIERS.length;

/** Custom type -> slot for the graph on screen. Set by assignCustomTypes; a type not in it falls back to its hash. */
let customSlots = new Map<string, number>();

/** Gives each name its own slot of `slots`: by hash, with linear probing in name order, so a clash moves only one
 * name. Past `slots` names, slots repeat. */
function probe(names: string[], slots: number): Map<string, number> {
  const used = new Array<boolean>(slots).fill(false);
  const out = new Map<string, number>();
  for (const name of [...names].sort()) {
    const start = hash(name) % slots;
    let slot = start;
    for (let i = 0; i < slots && used[slot]; i++) slot = (start + i + 1) % slots;
    used[slot] = true;
    out.set(name, slot);
  }
  return out;
}

/** Gives each custom type in a graph its own slot (probe). */
export function assignCustomTypes(types: Iterable<string>): Map<string, number> {
  customSlots = probe([...new Set(types)].filter((t) => !isStdType(t)), SLOTS);
  return customSlots;
}

function customSlot(type: string): number {
  return customSlots.get(type) ?? hash(type) % SLOTS;
}

export function typeHue(type: string): number {
  return Object.hasOwn(STD, type) ? STD[type].hue : CUSTOM_HUES[customSlot(type) % CUSTOM_HUES.length];
}

function typeLightness(type: string, theme: Theme): number {
  if (Object.hasOwn(STD, type)) return TONE[theme].l;
  return TONE[theme].l + TIERS[Math.floor(customSlot(type) / CUSTOM_HUES.length)];
}

function linearToSrgb(v: number): number {
  return v <= 0.0031308 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.4) - 0.055;
}

function oklchToLinear(l: number, c: number, hDeg: number): [number, number, number] {
  const h = (hDeg * Math.PI) / 180;
  const a = c * Math.cos(h);
  const b = c * Math.sin(h);
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
    -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
  ];
}

/** OKLCH to #rrggbb, lowering chroma until the colour fits in sRGB. */
export function oklchToHex(l: number, c: number, h: number): string {
  let chroma = c;
  let rgb = oklchToLinear(l, chroma, h);
  while (rgb.some((v) => v < -1e-4 || v > 1 + 1e-4) && chroma > 0) {
    chroma = Math.max(0, chroma - 0.005);
    rgb = oklchToLinear(l, chroma, h);
  }
  return (
    "#" +
    rgb
      .map((v) => Math.round(Math.min(1, Math.max(0, linearToSrgb(Math.min(1, Math.max(0, v))))) * 255))
      .map((v) => v.toString(16).padStart(2, "0"))
      .join("")
  );
}

export function typeColor(type: string, theme: Theme, muted = false): string {
  const tone = TONE[theme];
  const family = Object.hasOwn(STD, type) ? STD[type] : undefined;
  const chroma = (family?.chroma ?? tone.c) * (muted ? 0.4 : 1);
  return oklchToHex(typeLightness(type, theme), chroma, typeHue(type));
}

/** Groups smaller than this get no hull and the neutral colour. */
export const HULL_MIN = 4;
const GOLDEN_DEG = 137.508;

/** Hue of the group ranked `rank` by size (0 is the biggest): golden-angle steps, so neighbours in rank differ. */
export function groupHue(rank: number): number {
  return (25 + rank * GOLDEN_DEG) % 360;
}

const NEUTRAL: Record<Theme, string> = { light: "#a8a39c", dark: "#77736f", console: "#4d6878", instrument: "#94a3ab" };

/** Fill for a node coloured by its group; small groups and hidden-out nodes (rank < 0) are neutral. */
export function groupColor(rank: number, size: number, theme: Theme, muted = false): string {
  if (rank < 0 || size < HULL_MIN) return NEUTRAL[theme];
  const tone = TONE[theme];
  return oklchToHex(tone.l, tone.c * (muted ? 0.4 : 1), groupHue(rank));
}

/** Mixes `color` into `base` at `amount` (0..1), both #rrggbb: an opaque tint, since the hull layer draws opaque. */
export function mix(base: string, color: string, amount: number): string {
  const parse = (c: string) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
  const a = parse(base);
  const b = parse(color);
  return "#" + a.map((v, i) => Math.round(v + (b[i] - v) * amount).toString(16).padStart(2, "0")).join("");
}

/** Hull tint strength per theme (design note section 1). */
export const HULL_TINT: Record<Theme, number> = { light: 0.08, dark: 0.12, console: 0.1, instrument: 0.08 };

export type StatusLook = { ring: string; ringSize: number; muted: boolean; kind: StatusKind };
export type StatusKind = "open" | "tentative" | "settled" | "blocked" | "other";

const TENTATIVE = new Set(["proposed", "draft", "leaning"]);
const SETTLED = new Set(["agreed", "done", "closed", "acknowledged", "resolved"]);

export function statusKind(status: string): StatusKind {
  if (status === "open") return "open";
  if (TENTATIVE.has(status)) return "tentative";
  if (SETTLED.has(status)) return "settled";
  if (status === "blocked") return "blocked";
  return "other";
}

// Settled work recedes (thin, low contrast on the muted fill); blocked is the loud one (ui-expert N2). In the FUI
// skins open is the data level, tentative and other the structure level, settled near the ground.
const RINGS: Record<Theme, Record<StatusKind, string>> = {
  light: { open: "#4a4744", tentative: "#b3aea7", settled: "#d3cec7", blocked: "#d62839", other: "#8c8781" },
  dark: { open: "#c9c6c2", tentative: "#6c6966", settled: "#3b3936", blocked: "#ff5c6c", other: "#8f8b87" },
  console: { open: "#d6f0ff", tentative: "#4d7184", settled: "#16303d", blocked: "#ff5c6c", other: "#7fa6ba" },
  instrument: { open: "#16232b", tentative: "#9aabb4", settled: "#d0d9dd", blocked: "#d62839", other: "#4f6470" },
};

const RING_SIZE: Record<StatusKind, number> = { open: 0.16, tentative: 0.09, settled: 0.07, blocked: 0.26, other: 0.14 };

export function statusLook(status: string, theme: Theme): StatusLook {
  const kind = statusKind(status);
  return { ring: RINGS[theme][kind], ringSize: RING_SIZE[kind], muted: kind === "settled", kind };
}

const STATUS_HUE: Record<StatusKind, number | null> = { open: 250, tentative: 80, settled: 150, blocked: 25, other: null };

/** Fill when colouring by status: blue open, amber tentative, muted green settled, red blocked, neutral other. */
export function statusColor(kind: StatusKind, theme: Theme): string {
  const hue = STATUS_HUE[kind];
  if (hue === null) return NEUTRAL[theme];
  const tone = TONE[theme];
  return oklchToHex(tone.l, tone.c * (kind === "settled" ? 0.55 : 1), hue);
}

export const STATUS_KINDS: [StatusKind, string][] = [
  ["open", "open"],
  ["tentative", "proposed, draft, leaning"],
  ["settled", "agreed, done, closed"],
  ["blocked", "blocked"],
  ["other", "anything else"],
];

/** Node size from degree: log scale, clamped. */
export function nodeSize(degree: number, order: number): number {
  const base = order > 5000 ? 1.4 : order > 1000 ? 2.4 : 4.5;
  const step = order > 5000 ? 0.9 : order > 1000 ? 1.3 : 1.8;
  return Math.min(base * 4, base + step * Math.log2(1 + degree));
}

/** Relation hues (ui-expert N11): their own palette of 10 slots, outside the red band, at low chroma, so a
 * relation never reads as a type fill. */
export const RELATION_HUES: readonly number[] = Array.from({ length: 10 }, (_, i) => 40 + i * 32);
export const RELATION_CHROMA = 0.06;

/** Relation -> slot for the graph on screen; set by assignRelations. */
let relationSlots = new Map<string, number>();

/** Gives each relation in a graph its own slot, by hash with probing in name order, like custom types. */
export function assignRelations(relations: Iterable<string>): Map<string, number> {
  relationSlots = probe([...new Set(relations)], RELATION_HUES.length);
  return relationSlots;
}

export function relationHue(relation: string): number {
  return RELATION_HUES[relationSlots.get(relation) ?? hash(relation) % RELATION_HUES.length];
}

/** Edge colour for a relation in a focused neighbourhood (design note section 4). part_of is not coloured here:
 * it keeps the structure colour. */
export function relationColor(relation: string, theme: Theme): string {
  const tone = TONE[theme];
  return oklchToHex(ground(theme) === "light" ? tone.l - 0.08 : tone.l, RELATION_CHROMA, relationHue(relation));
}

// The FUI skins' canvas pieces (K1, ui-expert design note sections 4 to 6), pure so they are tested directly:
// callout text, leader placement, which nodes glow, and the finite fades. Graph text is only ever text here: it is
// cleaned, capped by grapheme and isolated, then drawn with fillText (security K1-b).

/** At most this many callouts on screen (readability rule 5); the rest are counted as +n. */
export const LEADER_CAP = 12;
/** Callout titles and notes are capped at this many graphemes. */
export const CALLOUT_CHARS = 40;
/** At most this many glowing nodes; agent highlights past it keep their rings only. */
export const GLOW_CAP = 200;

// C0 and C1 controls, line and paragraph separators, and every bidi control (embeddings, overrides, isolates and
// the marks), so a label can neither break lines nor reorder what is drawn around it. Each becomes a space.
function stripped(c: number): boolean {
  return (
    c < 0x20 || (c >= 0x7f && c <= 0x9f) || c === 0x061c || c === 0x200e || c === 0x200f || c === 0x2028 || c === 0x2029 ||
    (c >= 0x202a && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069)
  );
}
// Zero-width space, non-joiner, joiner and the BOM (security N1): dropped with no space, so they cannot
// hide text or split a word. An emoji joined by U+200D draws as its parts.
function zeroWidth(c: number): boolean {
  return (c >= 0x200b && c <= 0x200d) || c === 0xfeff;
}

const FSI = "\u2068";
const PDI = "\u2069";

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/**
 * Graph text for a callout: controls, bidi controls and zero-width characters dropped, capped at `max` graphemes (the last one an
 * ellipsis), then wrapped in one first-strong isolate. The cap counts graphemes, so it never splits a grapheme, and
 * the isolate is added after the cap, so it is always closed.
 */
export function calloutText(text: string, max = CALLOUT_CHARS): string {
  return isolate(cleanGraphemes(text), max);
}

/** The text's graphemes after dropping controls, bidi controls and zero-width characters, with runs of space as one. */
function cleanGraphemes(text: string): string[] {
  let spaced = "";
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (!zeroWidth(c)) spaced += stripped(c) ? " " : text[i];
  }
  const clean = spaced.replace(/\s+/g, " ").trim();
  return [...graphemes.segment(clean)].map((s) => s.segment);
}

/** At most `max` graphemes, the last an ellipsis when cut, in one closed isolate. */
function isolate(parts: readonly string[], max: number): string {
  const capped = parts.length > max ? `${parts.slice(0, Math.max(0, max - 1)).join("").trimEnd()}…` : parts.join("");
  return `${FSI}${capped}${PDI}`;
}

/**
 * calloutText, then cut further at a grapheme boundary until it measures at most `width` pixels (review B1:
 * 40 wide glyphs are wider than any box). Never splits a grapheme or the isolate; may end as just the ellipsis.
 */
export function fitCallout(text: string, width: number, measure: (s: string) => number, max = CALLOUT_CHARS): string {
  const parts = cleanGraphemes(text);
  const whole = isolate(parts, max);
  if (measure(whole) <= width) return whole;
  const cut = (k: number) => `${FSI}${parts.slice(0, k).join("").trimEnd()}…${PDI}`;
  // The most graphemes before the ellipsis that fit; the width grows with the count.
  let lo = 0;
  let hi = Math.min(parts.length, max) - 1;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measure(cut(mid)) <= width) lo = mid;
    else hi = mid - 1;
  }
  return cut(lo);
}

/** A callout box is at most this wide, and never wider than its half of the space between the columns' edges, so
 * the two columns cannot overlap on a narrow canvas (review B1). Below MIN_COLUMN there is no callout at all. */
export const CALLOUT_W = 300;
export const MIN_COLUMN = 80;
/** Each column's box sits this far in from its edge; the two columns keep this much between them. */
export const COLUMN_INSET = 16;

export function columnWidth(frame: Pick<Frame, "left" | "right">): number {
  return Math.min(CALLOUT_W, Math.floor((frame.right.edge - frame.left.edge - COLUMN_INSET * 3) / 2));
}

/** The focus the canvas draws and the readout names: the person's when on, else the agent's (review B3). */
export function effectiveFocus<F extends { depths: ReadonlyMap<string, number>; hops: number }>(person: F | null, agent: F | null): F | null {
  return person ?? agent;
}

export type LeaderKind = "select" | "agent";
/** `w`: the callout box's width (its text measured first); `r`: the node's radius on screen. */
export type LeaderCandidate = { id: string; kind: LeaderKind; x: number; y: number; h: number; w?: number; r?: number };
export type Placed = LeaderCandidate & { side: "left" | "right"; top: number };
export type Frame = {
  width: number;
  height: number;
  /** Each column's outer x (the canvas edge, or the node panel's), and the space kept clear at its top and bottom
   * (readout card, agent strip, zoom buttons). */
  left: { top: number; bottom: number; edge: number };
  right: { top: number; bottom: number; edge: number };
  gap: number;
  /** A node no box may cover: the selected one (ui-expert on K1-N1). */
  keepClear?: { x: number; y: number; r: number } | null;
};

/** How far a box keeps from a node's disc. */
export const CLEAR_PX = 4;
type Disc = { x: number; y: number; r: number };
const boxX = (frame: Frame, side: "left" | "right", w: number) => (side === "left" ? frame.left.edge + COLUMN_INSET : frame.right.edge - COLUMN_INSET - w);
/** Whether a box at x, w across the column covers a disc anywhere along it (the x bands only). */
const bandHits = (x: number, w: number, d: Disc) => d.x + d.r + CLEAR_PX > x && d.x - d.r - CLEAR_PX < x + w;
/** Whether the box covers the disc (with CLEAR_PX). */
function boxHits(x: number, top: number, w: number, h: number, d: Disc): boolean {
  const nx = Math.min(Math.max(d.x, x), x + w);
  const ny = Math.min(Math.max(d.y, top), top + h);
  return Math.hypot(d.x - nx, d.y - ny) < d.r + CLEAR_PX;
}

/** The candidates that can have a callout: the first of each id, inside the frame, up to LEADER_CAP; `beyond` counts
 * the other ones inside. The canvas measures the text of these only (reviewer N1). */
export function leaderChoice<C extends LeaderCandidate>(candidates: readonly C[], frame: Frame): { chosen: C[]; beyond: number } {
  const seen = new Set<string>();
  const inside = candidates.filter((c) => {
    if (seen.has(c.id)) return false;
    seen.add(c.id);
    return c.x >= frame.left.edge && c.y >= 0 && c.x <= frame.right.edge && c.y <= frame.height;
  });
  return { chosen: inside.slice(0, LEADER_CAP), beyond: Math.max(0, inside.length - LEADER_CAP) };
}

/**
 * Callouts in a column at the canvas edge nearest each node, in candidate order (selection first, then agent nodes
 * by event order) up to LEADER_CAP. Only nodes inside the frame get one. Within a column the boxes keep the nodes'
 * vertical order and never overlap; a callout that fits in neither column is counted in `more`, like every
 * candidate past the cap. No box covers its own node or the selected node (ui-expert K1-N1): a box whose
 * column would cover its node goes to the other column; if that covers it too it stays and moves past the node, the
 * column is stacked again, and a box that still covers a node it must not is counted in `more`.
 */
export function placeLeaders(candidates: readonly LeaderCandidate[], frame: Frame): { placed: Placed[]; more: number } {
  const { chosen, beyond } = leaderChoice(candidates, frame);
  const colW = columnWidth(frame);
  if (colW < MIN_COLUMN) return { placed: [], more: chosen.length + beyond };
  let more = beyond;
  const room = (side: "left" | "right") => frame.height - frame[side].top - frame[side].bottom;
  const mid = (frame.left.edge + frame.right.edge) / 2;
  const width = (c: LeaderCandidate) => Math.min(c.w ?? colW, colW);
  const own = (c: LeaderCandidate): Disc => ({ x: c.x, y: c.y, r: c.r ?? 0 });
  const columns: Record<"left" | "right", LeaderCandidate[]> = { left: [], right: [] };
  const used = { left: 0, right: 0 };
  for (const c of chosen) {
    let near: "left" | "right" = c.x < mid ? "left" : "right";
    let far: "left" | "right" = near === "left" ? "right" : "left";
    const covers = (side: "left" | "right") => bandHits(boxX(frame, side, width(c)), width(c), own(c));
    if (covers(near) && !covers(far)) [near, far] = [far, near];
    const side = used[near] + c.h <= room(near) ? near : used[far] + c.h <= room(far) ? far : null;
    if (!side) {
      more++;
      continue;
    }
    columns[side].push(c);
    used[side] += c.h + frame.gap;
  }
  const placed: Placed[] = [];
  for (const side of ["left", "right"] as const) {
    let list = [...columns[side]];
    const top = frame[side].top;
    const bottom = frame.height - frame[side].bottom;
    const discs = (c: LeaderCandidate) => [own(c), ...(frame.keepClear ? [frame.keepClear] : [])];
    // Each box as near its node as it can be, moved past a node it would cover, pushed down past the one above,
    // then back up from the bottom.
    const stack = (cs: LeaderCandidate[]) => {
      const want = cs.map((c) => {
        const x = boxX(frame, side, width(c));
        let t = c.y - c.h / 2;
        for (const d of discs(c)) {
          if (!boxHits(x, t, width(c), c.h, d)) continue;
          const [up, down] = [d.y - d.r - CLEAR_PX - c.h, d.y + d.r + CLEAR_PX];
          t = Math.abs(up - t) <= Math.abs(down - t) && up >= top ? up : down;
        }
        return t;
      });
      const order = cs.map((c, i) => ({ c, t: want[i] })).sort((a, b) => a.t - b.t);
      const tops = order.map(({ c, t }) => Math.min(Math.max(t, top), bottom - c.h));
      for (let i = 1; i < tops.length; i++) tops[i] = Math.max(tops[i], tops[i - 1] + order[i - 1].c.h + frame.gap);
      for (let i = tops.length - 1; i >= 0; i--) {
        const limit = i === tops.length - 1 ? bottom - order[i].c.h : tops[i + 1] - frame.gap - order[i].c.h;
        tops[i] = Math.max(top, Math.min(tops[i], limit));
      }
      return order.map(({ c }, i) => ({ c, top: tops[i] }));
    };
    let laid = stack(list);
    // A box that still covers a node it must not is left out (counted), and the column is stacked again.
    for (;;) {
      const bad = laid.find(({ c, top: t }) => discs(c).some((d) => boxHits(boxX(frame, side, width(c)), t, width(c), c.h, d)));
      if (!bad) break;
      list = list.filter((c) => c !== bad.c);
      more++;
      laid = stack(list);
    }
    for (const { c, top: t } of laid) placed.push({ ...c, side, top: t });
  }
  return { placed, more };
}

export type GlowKind = "select" | "agent" | "live";

/**
 * The nodes that glow and why (readability rule 2): the person's selection, the agent's highlight and focus target,
 * and a live change; nothing else. A node glows once, for its first reason in that order; at most GLOW_CAP glow.
 */
export function glowSet(
  selection: Iterable<string>,
  agent: Iterable<string>,
  live: Iterable<string>,
  shown: (id: string) => boolean,
): { glow: [string, GlowKind][]; wanted: number } {
  const out = new Map<string, GlowKind>();
  for (const [ids, kind] of [[selection, "select"], [agent, "agent"], [live, "live"]] as const) {
    for (const id of ids) if (!out.has(id) && shown(id)) out.set(id, kind);
  }
  const all = [...out.entries()];
  return { glow: all.slice(0, GLOW_CAP), wanted: all.length };
}

/** live-glow: 600 ms in, 1.2 s hold, 600 ms out, once per change. */
export const LIVE_IN = 600;
export const LIVE_HOLD = 1200;
export const LIVE_OUT = 600;
export const LIVE_TOTAL = LIVE_IN + LIVE_HOLD + LIVE_OUT;

/** The live glow's strength `t` ms after a change; with reduced motion it is on, unfaded, for the same time. */
export function liveGlow(t: number, reduced: boolean): number {
  if (t < 0 || t >= LIVE_TOTAL) return 0;
  if (reduced) return 1;
  if (t < LIVE_IN) return t / LIVE_IN;
  if (t < LIVE_IN + LIVE_HOLD) return 1;
  return 1 - (t - LIVE_IN - LIVE_HOLD) / LIVE_OUT;
}

/** callout-in: 400 ms, the leader grows in the first half and the text comes in the second. */
export const CALLOUT_MS = 400;

export function calloutIn(t: number, reduced: boolean): { leader: number; text: number } {
  if (reduced || t >= CALLOUT_MS) return { leader: 1, text: 1 };
  const p = Math.max(0, t) / CALLOUT_MS;
  return { leader: Math.min(1, p * 2), text: Math.max(0, p * 2 - 1) };
}

/** #rrggbb with an alpha, for canvas gradients. */
export function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return `rgba(${r},${g},${b},${Math.max(0, Math.min(1, alpha)).toFixed(3)})`;
}

/** A glow's radii in Console and the FUI skins: it starts at the node's edge, not its centre, so a filled node never
 * turns into a blob of glow at 10k (ui-expert K1-N4). */
export function glowRadii(r: number): { inner: number; outer: number } {
  return { inner: r, outer: Math.max(r * 3, 8) };
}

/** Where a leader meets its callout: the point of the box nearest the node (ui-expert K1-N1). */
export function leaderEnd(node: { x: number; y: number }, box: { x: number; y: number; w: number; h: number }): { x: number; y: number } {
  return { x: Math.min(Math.max(node.x, box.x), box.x + box.w), y: Math.min(Math.max(node.y, box.y), box.y + box.h) };
}

/** The readout over the callout column for the callouts left out (ui-expert K1-N3). */
export const moreCallouts = (more: number) => `CALLOUTS +${more}`;

/** At most this many cluster pills in the FUI skins, biggest group first, as callouts are capped (ui-expert K1-N2). */
export const FUI_PILLS = 12;

/** A cluster pill's look: in the FUI skins quiet, its text at structure level and its border chrome-hi, at most
 * FUI_PILLS of them, so at 10k they are not the brightest thing in Console (ui-expert K1-N2); elsewhere the
 * ink and the group's colour, at most `cap`. */
export function pillLook(fui: boolean, palette: { ink: string; structure: string; chromeHi: string }, groupColour: string, cap: number): { text: string; border: string; cap: number } {
  return fui ? { text: palette.structure, border: palette.chromeHi, cap: Math.min(cap, FUI_PILLS) } : { text: palette.ink, border: groupColour, cap };
}

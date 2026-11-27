// Edges on the canvas (ws009 E1, plan 150; ui-expert design 2026-11-12): which way an edge points, its relation's name
// as a label, and the quiet context relations. Pure, so tests run every rule directly; the canvas only wires it.
import type { ViewGraph } from "@/lib/graph/build";
import { relationColor, type Theme } from "@/lib/graph/colors";
import { aliasOfKey } from "@/lib/graph/keys";

/** Context relations drawn quietly by default (D42: the UI's list, never the vocabulary's). */
export const QUIET_RELATIONS: ReadonlySet<string> = new Set(["part_of", "raised_in", "raised_by", "sourced_from", "mentions", "attended", "held_at"]);

/** Symmetric in every graph (umx docs get ontology, D49), and similar_to: drawn as plain lines, never with heads. A
 * graph's own symmetric relations come from `umx ontology show` with its snapshot. */
export const ALWAYS_SYMMETRIC: ReadonlySet<string> = new Set(["conflicts_with", "alternative_to", "possibly_same_as", "similar_to"]);

/** How a relation's edges are drawn: quiet (thin, structure colour, small heads, labels only when forced), normal, or
 * off (not drawn; the filter chip's "off", Filters.relations). */
export type EdgeMode = "quiet" | "normal" | "off";

/** What the person set on a relation chip: quiet or normal; anything not set takes the default (quiet for the list). */
export type RelationModes = ReadonlyMap<string, "quiet" | "normal">;

export function defaultMode(relation: string): "quiet" | "normal" {
  return QUIET_RELATIONS.has(relation) ? "quiet" : "normal";
}

/** A relation's mode now: off when its chip is off, else the person's choice, else its default. */
export function relationMode(relation: string, modes: RelationModes, off: ReadonlySet<string>): EdgeMode {
  if (off.has(relation)) return "off";
  return modes.get(relation) ?? defaultMode(relation);
}

/** A chip click: quiet, then normal, then off, then quiet again. */
export function nextMode(mode: EdgeMode): EdgeMode {
  return mode === "quiet" ? "normal" : mode === "normal" ? "off" : "quiet";
}

/** The chip's next state as the two things that hold it: whether it is off, and the mode the person chose. */
export function cycleRelation(relation: string, modes: RelationModes, off: ReadonlySet<string>): { off: boolean; modes: RelationModes } {
  const next = nextMode(relationMode(relation, modes, off));
  if (next === "off") return { off: true, modes };
  const out = new Map(modes);
  if (next === defaultMode(relation)) out.delete(relation);
  else out.set(relation, next);
  return { off: false, modes: out };
}

/** Whether `relation` is symmetric in the graph that stores `edge` (its key's alias). */
export function isSymmetric(edge: string, relation: string, symmetric: ReadonlyMap<string, ReadonlySet<string>>): boolean {
  return ALWAYS_SYMMETRIC.has(relation) || (symmetric.get(aliasOfKey(edge))?.has(relation) ?? false);
}

/** A relation chip's swatch: a plain line when the relation is symmetric in any open graph. */
export function relationSymmetric(relation: string, symmetric: ReadonlyMap<string, ReadonlySet<string>>): boolean {
  if (ALWAYS_SYMMETRIC.has(relation)) return true;
  for (const set of symmetric.values()) if (set.has(relation)) return true;
  return false;
}

/** Whether an edge is drawn and not dimmed: its relation on, both ends shown, and inside the focus when there is one.
 * Only these can have their labels forced on. */
export function edgeDrawn(graph: ViewGraph, off: ReadonlySet<string>, focused: ReadonlyMap<string, number> | null): (edge: string) => boolean {
  return (edge) => {
    const [s, t] = graph.extremities(edge);
    return !off.has(graph.getEdgeAttribute(edge, "kind")) && !graph.getNodeAttribute(s, "hidden") && !graph.getNodeAttribute(t, "hidden") && (!focused || (focused.has(s) && focused.has(t)));
  };
}

/** The sigma edge program for an edge: arrows point source to target, symmetric relations are plain lines; curved
 * when it is one of a parallel or opposite pair. */
export type EdgeProgram = "arrow" | "line" | "curvedArrow" | "curve";

export function edgeProgram(symmetric: boolean, curvature: number): EdgeProgram {
  if (curvature === 0) return symmetric ? "line" : "arrow";
  return symmetric ? "curve" : "curvedArrow";
}

/** Screen px (design point 1). */
export const EDGE_SIZE = { normal: 1.2, quiet: 0.7, emphasis: 1.8 } as const;

/** How far apart two edges of one pair bend (edge-curve's curvature, a share of the edge's length). */
export const CURVE_STEP = 0.25;

/**
 * Curvature for every edge that shares its two nodes with another (parallel or opposite), so none hides under another:
 * per pair of nodes, edges in key order spread evenly around the straight line; an edge drawn the other way round
 * takes the opposite sign, since a curve bends relative to its own direction. An edge alone on its pair is 0 (straight),
 * so a pair that loses an edge straightens.
 */
export function curvatures(graph: ViewGraph): Map<string, number> {
  const pairs = new Map<string, { edge: string; flipped: boolean }[]>();
  graph.forEachEdge((edge, _a, s, t) => {
    if (s === t) return;
    const flipped = s > t;
    const key = flipped ? `${t}\u0000${s}` : `${s}\u0000${t}`;
    const list = pairs.get(key);
    if (list) list.push({ edge, flipped });
    else pairs.set(key, [{ edge, flipped }]);
  });
  const out = new Map<string, number>();
  for (const list of pairs.values()) {
    if (list.length < 2) continue;
    list.sort((a, b) => (a.edge < b.edge ? -1 : a.edge > b.edge ? 1 : 0));
    const mid = (list.length - 1) / 2;
    list.forEach(({ edge, flipped }, i) => {
      // Two edges sit either side of the straight line; three or more spread from it.
      const c = list.length === 2 ? (i === 0 ? -0.5 : 0.5) * CURVE_STEP * 2 : (i - mid) * CURVE_STEP;
      if (c !== 0) out.set(edge, flipped ? -c : c);
    });
  }
  return out;
}

/** The most edge labels forced on for the selected node and the focus centre together (plan 198, the UI expert's
 * design note, section 2); the selected node's rest are counted in the panel ("+n edges"). */
export const FORCED_LABELS_MAX = 150;
/** The most of the hovered node's edges forced on, on top of those. */
export const HOVER_LABELS_MAX = 40;

/**
 * The edges whose labels are forced on, in the design's order (selected node, then the hovered node, then focus hop
 * 1), each shown edge once: the selected node's and the focus centre's at most `max` together, the hovered node's at
 * most `hoverMax` more. `more` counts the selected node's shown edges left without one.
 */
export function forcedLabels(
  graph: ViewGraph,
  at: { selected: string | null; hovered: string | null; focusCentre: string | null; members?: readonly string[] },
  shown: (edge: string) => boolean,
  max = FORCED_LABELS_MAX,
  hoverMax = HOVER_LABELS_MAX,
): { edges: Set<string>; more: number } {
  const edges = new Set<string>();
  let more = 0;
  let pinned = 0;
  let hovers = 0;
  const take = (node: string | null, kind: "selected" | "hovered" | "focus") => {
    if (!node || !graph.hasNode(node)) return;
    graph.forEachEdge(node, (edge) => {
      if (edges.has(edge) || !shown(edge)) return;
      if (kind === "hovered") {
        if (hovers < hoverMax) {
          edges.add(edge);
          hovers++;
        }
      } else if (pinned < max) {
        edges.add(edge);
        pinned++;
      } else if (kind === "selected") more++;
    });
  };
  take(at.selected, "selected");
  // A selected merged entity (PRD 005 centre-14): every member's edges are the selection's.
  for (const m of at.members ?? []) take(m, "selected");
  take(at.hovered, "hovered");
  take(at.focusCentre, "focus");
  return { edges, more };
}

/** The edges whose labels may be forced on, and that the panel's "+n edges" counts: drawn (edgeDrawn) and not left
 * out by the edge reducer (`leftOut`, its own predicate: a denial, a threaded identity edge, a merged inner edge),
 * so a hidden edge never gets a floating label or takes a forced slot (reviewer R370-2). */
export function forcedShown(graph: ViewGraph, off: ReadonlySet<string>, focused: ReadonlyMap<string, number> | null, leftOut: (edge: string) => boolean): (edge: string) => boolean {
  const drawn = edgeDrawn(graph, off, focused);
  return (edge) => drawn(edge) && !leftOut(edge);
}

/** At most this many edges drawn, and every one is labelled, quiet relations included, with no fit test. */
export const SMALL_GRAPH_EDGES = 150;
/** The most edge labels drawn in one frame; forced labels are drawn first. */
export const LABELS_PER_FRAME = 300;
/** The zoom rule's clearance, in screen px, besides the label and both nodes. */
export const LABEL_GAP = 16;
/** The shortest edge on screen the zoom rule labels (ui-expert N1), in px. */
export const RULE_MIN_LENGTH = 80;

/** The zoom rule (design section 2): an edge is labelled once its length on screen holds its label, both node radii
 * and LABEL_GAP, and is at least RULE_MIN_LENGTH. */
export function fitsZoomRule(length: number, labelWidth: number, sourceRadius: number, targetRadius: number): boolean {
  return length >= RULE_MIN_LENGTH && length >= labelWidth + sourceRadius + targetRadius + LABEL_GAP;
}

/**
 * One frame's edge labels: the forced ones first, in their order, then the drawn edges, every one when there are at
 * most SMALL_GRAPH_EDGES (`every`), else by the zoom rule; the painters say whether they drew. Stops once `cap`
 * labels are drawn, so forced labels are never crowded out by the rule's.
 */
export function labelFrame<T extends { edge: string }>(
  forced: Iterable<string>,
  drawn: readonly T[],
  paint: { forced: (edge: string) => boolean; other: (item: T, every: boolean) => boolean },
  cap = LABELS_PER_FRAME,
): { forced: number; other: number } {
  const done = new Set<string>();
  let forcedCount = 0;
  for (const edge of forced) {
    if (forcedCount >= cap) break;
    done.add(edge);
    if (paint.forced(edge)) forcedCount++;
  }
  const every = drawn.length <= SMALL_GRAPH_EDGES;
  let other = 0;
  for (const item of drawn) {
    if (forcedCount + other >= cap) break;
    if (!done.has(item.edge) && paint.other(item, every)) other++;
  }
  return { forced: forcedCount, other };
}

type Point = { x: number; y: number };

/** Whether an edge reads left to right on screen (straight down when vertical): its label is drawn source to target. */
export function readsForward(s: Point, t: Point): boolean {
  return t.x > s.x || (t.x === s.x && t.y >= s.y);
}

/** A direction chevron on the label's side the edge points to: the text stays upright, so an edge drawn right to left
 * on screen reads "‹ relation". */
export function labelText(relation: string, s: Point, t: Point): string {
  return readsForward(s, t) ? `${relation} ›` : `‹ ${relation}`;
}

/** Where an edge's label sits: the middle of the line, or of the curve (edge-curve's quadratic, its apex halfway to
 * the control point at centre + curvature (dy, -dx), in viewport coordinates). */
export function labelAnchor(s: Point, t: Point, curvature: number): Point {
  const [cx, cy] = [(s.x + t.x) / 2, (s.y + t.y) / 2];
  if (!curvature) return { x: cx, y: cy };
  const [dx, dy] = [t.x - s.x, t.y - s.y];
  return { x: cx + 0.5 * curvature * dy, y: cy - 0.5 * curvature * dx };
}

/** A screen box, in viewport px. */
export type LabelBox = { x0: number; y0: number; x1: number; y1: number };
export const overlaps = (a: LabelBox, b: LabelBox) => a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;

/** Where sigma's disc label (drawDiscNodeLabel) puts a node's label: 3 px right of the node, text `labelPx` high on a
 * baseline a third of it below the centre. */
export function nodeLabelBox(x: number, y: number, size: number, labelPx: number, width: number): LabelBox {
  const baseline = y + labelPx / 3;
  return { x0: x + size + 3, y0: baseline - 0.8 * labelPx, x1: x + size + 3 + width, y1: baseline + 0.25 * labelPx };
}

/** The screen box around an edge label `width` wide, centred at `at` and turned by `angle`, its halo included. */
export function edgeLabelBox(at: Point, angle: number, width: number): LabelBox {
  const [hw, hh] = [width / 2 + 1.5, LABEL_PX / 2 + 1.5];
  const [c, s] = [Math.abs(Math.cos(angle)), Math.abs(Math.sin(angle))];
  const [dx, dy] = [hw * c + hh * s, hw * s + hh * c];
  return { x0: at.x - dx, y0: at.y - dy, x1: at.x + dx, y1: at.y + dy };
}

/** The label's look (design point 2): 11 px, sans or mono by theme, a 3 px halo in the canvas colour, no box. */
export type LabelStyle = { font: string; color: string; halo: string };
export const LABEL_PX = 11;

/**
 * Paints one edge label: relation and chevron, upright, along the edge, at the middle of its line or curve. `fit`
 * skips a label the zoom rule does not fit (fitsZoomRule), and one whose box `avoid` says meets a drawn node label
 * (node labels first, ui-expert E1-N3); forced labels and a small graph's are drawn wherever the edge has room.
 * `measure`, when given, is the text's width in this font (a cache the caller keeps per frame font); `stamp` draws the
 * text and its halo centred at the origin (a cached image, which costs far less than stroking the text each frame).
 */
export function paintEdgeLabel(
  ctx: Pick<CanvasRenderingContext2D, "save" | "restore" | "translate" | "rotate" | "measureText" | "strokeText" | "fillText"> & {
    font: string; fillStyle: unknown; strokeStyle: unknown; lineWidth: number; lineJoin: CanvasLineJoin; textAlign: CanvasTextAlign; textBaseline: CanvasTextBaseline;
  },
  relation: string,
  s: Point & { size: number },
  t: Point & { size: number },
  curvature: number,
  style: LabelStyle,
  fit: boolean,
  avoid?: (box: LabelBox) => boolean,
  measure?: (text: string) => number,
  stamp?: (text: string, style: LabelStyle) => void,
): boolean {
  const length = Math.hypot(t.x - s.x, t.y - s.y);
  const room = length - s.size - t.size;
  if (room <= 0) return false;
  const text = labelText(relation, s, t);
  ctx.font = `500 ${LABEL_PX}px ${style.font}`;
  const width = measure ? measure(text) : ctx.measureText(text).width;
  if (fit && !fitsZoomRule(length, width, s.size, t.size)) return false;
  const [a, b] = readsForward(s, t) ? [s, t] : [t, s];
  const at = labelAnchor(s, t, curvature);
  if (fit && avoid?.(edgeLabelBox(at, Math.atan2(b.y - a.y, b.x - a.x), width))) return false;
  ctx.save();
  ctx.translate(at.x, at.y);
  ctx.rotate(Math.atan2(b.y - a.y, b.x - a.x));
  if (stamp) stamp(text, style);
  else {
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.lineJoin = "round";
    ctx.lineWidth = 3;
    ctx.strokeStyle = style.halo;
    ctx.strokeText(text, 0, 0);
    ctx.fillStyle = style.color;
    ctx.fillText(text, 0, 0);
  }
  ctx.restore();
  return true;
}

/** Each open graph's symmetric relations, by alias (from its snapshot's `umx ontology show`). */
export function symmetricByGraph(snaps: readonly { graph: string; symmetric?: readonly string[] }[]): Map<string, ReadonlySet<string>> {
  return new Map(snaps.map((s) => [s.graph, new Set(s.symmetric ?? [])]));
}

/** Arrowheads (plan 150): length and width as ratios of the edge's screen width. */
export const HEAD = { lengthToThicknessRatio: 5, widenessToThicknessRatio: 4 } as const;
/** Into a hub (more than HUB_IN drawn incoming edges), a head is at most 6 x 4.8 px, the normal edge's head, whatever
 * the edge's width (ui-expert's rule for E1-N1): an emphasis edge keeps its 1.8 px but takes these ratios. */
export const HUB_IN = 20;
export const HUB_HEAD = {
  lengthToThicknessRatio: (HEAD.lengthToThicknessRatio * EDGE_SIZE.normal) / EDGE_SIZE.emphasis,
  widenessToThicknessRatio: (HEAD.widenessToThicknessRatio * EDGE_SIZE.normal) / EDGE_SIZE.emphasis,
} as const;
export type HeadProgram = EdgeProgram | "arrowHub" | "curvedArrowHub";

/** Each node's drawn incoming edges: both ends shown and not left out by the edge reducer (`leftOut`, the reducer's own
 * predicate). Dimmed edges are drawn, so a focus does not change the count. */
export function incomingDrawn(graph: ViewGraph, leftOut: (edge: string) => boolean): Map<string, number> {
  const out = new Map<string, number>();
  graph.forEachEdge((edge, _a, s, t) => {
    if (graph.getNodeAttribute(s, "hidden") || graph.getNodeAttribute(t, "hidden") || leftOut(edge)) return;
    out.set(t, (out.get(t) ?? 0) + 1);
  });
  return out;
}

/** The program an edge into `target` is drawn with: a smaller head into a hub once the edge is wider than normal. */
export function headProgram(program: EdgeProgram, size: number, incoming: number): HeadProgram {
  if (incoming <= HUB_IN || size <= EDGE_SIZE.normal) return program;
  if (program === "arrow") return "arrowHub";
  if (program === "curvedArrow") return "curvedArrowHub";
  return program;
}

/** The selected node's edges (plan 150): the focus colour, ink in Instrument; in Console each relation's own colour,
 * so a selected hub reads by relation, not as one amber fan (ui-expert E1-N2); part_of in the structure colour,
 * as focus draws it. */
export function selectedEdgeColor(kind: string, palette: { theme: Theme; ink: string; edgeFocus: string; edgeStructure: string }): string {
  if (palette.theme === "instrument") return palette.ink;
  if (palette.theme === "console") return kind === "part_of" ? palette.edgeStructure : relationColor(kind, palette.theme);
  return palette.edgeFocus;
}

"use client";
import "@react-sigma/core/lib/style.css";
import { SigmaContainer, useRegisterEvents, useSetSettings, useSigma } from "@react-sigma/core";
import { createEdgeCurveProgram } from "@sigma/edge-curve";
import { Maximize, Minus, Plus } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { createEdgeArrowProgram, drawDiscNodeLabel } from "sigma/rendering";
import type { Settings } from "sigma/settings";
import type Sigma from "sigma";
import type { CameraState, MouseCoords, NodeDisplayData, PartialButFor } from "sigma/types";
import { type CanvasReadouts, Readouts, type ReadoutValues } from "@/components/readouts";
import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/tooltip";
import { canvasFit, type Gesture } from "@/lib/agent";
import { extentOf as graphExtent, type Extent } from "@/lib/graph/apply";
import type { EdgeAttrs, NodeAttrs, ViewGraph } from "@/lib/graph/build";
import { type Box, convexHull, placePills } from "@/lib/graph/clusters";
import { sizeRefresh } from "@/lib/graph/clearance";
import { curvatures, EDGE_SIZE, edgeProgram, fitsZoomRule, forcedLabels, forcedShown, HEAD, headProgram, HUB_HEAD, incomingDrawn, isSymmetric, labelAnchor, type LabelBox, labelFrame, LABEL_PX, type LabelStyle, nodeLabelBox, overlaps, paintEdgeLabel, type RelationModes, relationMode, selectedEdgeColor, SMALL_GRAPH_EDGES } from "@/lib/graph/edges";
import { groupColor, HULL_MIN, HULL_TINT, mix, relationColor, type Skin, type Theme } from "@/lib/graph/colors";
import { calloutIn, COLUMN_INSET, columnWidth, fitCallout, type Frame, glowRadii, glowSet, type LeaderCandidate, leaderChoice, leaderEnd, LIVE_TOTAL, liveGlow, moreCallouts, pillLook, placeLeaders, withAlpha } from "@/lib/graph/fui";
import { edgeLeftOut, type Entities, mergedPlaces, NO_ENTITIES, type Place, threadPairs } from "@/lib/graph/entities";
import { farEnd, type Pair, shadowAlpha } from "@/lib/graph/shadows";
import { ShapeProgram } from "@/lib/graph/shape-program";
import { extentOf, shapeAt, shapePath } from "@/lib/graph/shapes";
import { emphasis, SIMILAR_TO } from "@/lib/graph/focus";
import { nodesInLasso } from "@/lib/selection";
import { Lru } from "@/lib/lru";
import { cn } from "@/lib/utils";
import { anchorLabels, rowsLabelled, type SpineAxis } from "@/lib/graph/spine-layout";
import { SPACING } from "@/lib/graph/spacing";
import { hubSize, type LayoutMode } from "@/lib/graph/hubs";

export type CanvasPalette = {
  theme: Theme;
  skin: Skin;
  ink: string;
  /** The structure level: muted text. */
  structure: string;
  halo: string;
  canvas: string;
  edge: string;
  edgeStructure: string;
  edgeFocus: string;
  dim: string;
  edgeDim: string;
  accent: string;
  /** The agent's halo and badge: a cooler colour than the person's accent (design note section 5). */
  agent: string;
  font: string;
  mono: string;
  /** FUI chrome and glow tokens (K1). */
  chrome: string;
  chromeHi: string;
  chromeText: string;
  grid: string;
  glowSelect: string;
  glowAgent: string;
  glowLive: string;
};

/** A group's hull and label, from the group ranks the app sets on each node. `colors`, when given, is each group's
 * colour (grouping by graph, plan 133); otherwise the cluster palette's. `held`, grouping by graph in Hold, is each
 * graph's count of changes not applied yet ("12" or "many"), or null for none (plan 152). */
export type GroupInfo = { sizes: number[]; labels: string[]; heads: string[]; colors?: string[]; held?: (string | null)[] };

// Edge programs (plan 150, ui-expert design point 1): a head on every directed edge, stopping at the target's border,
// sized from the edge's screen width; plain lines for symmetric relations; curves only for parallel or opposite pairs.
// Edge labels are the canvas's own pass (drawEdgeLabels), so no program draws one.
const EdgePrograms = {
  arrow: createEdgeArrowProgram<NodeAttrs, EdgeAttrs>(HEAD),
  curvedArrow: createEdgeCurveProgram<NodeAttrs, EdgeAttrs>({ arrowHead: { extremity: "target", ...HEAD } }),
  // Into a hub, a wide edge's head stays the normal edge's size (E1-N1, lib/graph/edges headProgram).
  arrowHub: createEdgeArrowProgram<NodeAttrs, EdgeAttrs>(HUB_HEAD),
  curvedArrowHub: createEdgeCurveProgram<NodeAttrs, EdgeAttrs>({ arrowHead: { extremity: "target", ...HUB_HEAD } }),
  curve: createEdgeCurveProgram<NodeAttrs, EdgeAttrs>({ arrowHead: null }),
};

type Props = {
  graph: ViewGraph;
  palette: CanvasPalette;
  /** The node the panel shows and focus starts from. */
  selected: string | null;
  /** Every node the person selected: each gets the accent ring. */
  selection: ReadonlySet<string>;
  /** Move the camera to this node; n changes on every request. */
  jump: { id: string; n: number } | null;
  /** Node id -> hops from the selected node; everything else is dimmed. Null when focus is off. */
  focused: Map<string, number> | null;
  groups: GroupInfo | null;
  layoutDone: boolean;
  /** Relations filtered out: their edges are not drawn. */
  offRelations: ReadonlySet<string>;
  /** The relation chips' quiet or normal choices (lib/graph/edges); a relation not named takes its default. */
  relationModes?: RelationModes;
  /** Each open graph's symmetric relations by alias: drawn as lines, not arrows. */
  symmetric?: ReadonlyMap<string, ReadonlySet<string>>;
  /** Bumps after each visibility change, so a jump to a node just revealed runs once it is shown. */
  version: number;
  /** A click on a node; `add` when shift was held (add or remove it). */
  onPick: (id: string, add: boolean) => void;
  /** A plain click on the empty canvas. */
  onClear: () => void;
  /** A shift-drag lasso closed around these shown nodes. */
  onLasso: (ids: string[]) => void;
  /** Called once the camera has arrived at a jump's node. */
  onJumped?: (id: string) => void;
  /** Nodes an agent highlighted: a second, cooler halo and a small agent badge, never the selection's ring. */
  agentMarks: ReadonlySet<string>;
  /** Fit the camera to these nodes (none: the whole graph); n changes on every request. */
  fit: { nodes: string[]; n: number } | null;
  /** The person's use of the canvas, from press to release (lib/agent Gesture): an agent's camera move waits. */
  gesture?: Gesture;
  /** The agent's focus target, which glows and gets a callout in the FUI skins. */
  agentFocusNode?: string | null;
  /** The agent's note per node, from the newest highlight or focus that named it with one. */
  agentNotes?: ReadonlyMap<string, string>;
  /** The nodes the last live change touched, and when it was applied (performance.now()); n changes on every change. */
  live?: { n: number; ids: readonly string[]; at: number } | null;
  /** The readout card's numbers (FUI skins only). */
  readouts?: ReadoutValues | null;
  /** The node panel covers the right of the canvas: the right callout column and the readouts move left of it. */
  panelOpen?: boolean;
  /** An agent opened graphs (ws009 M1b1): sigma keeps framing the extent the canvas had, so nothing on screen moves. */
  hold?: Extent | null;
  /** The spine view: each graph's spine line (graph coordinates), drawn under the edges, earliest on the left. */
  spineAxes?: readonly SpineAxis[];
  /** A camera to take on this graph (Undo of an agent's open). */
  camera?: CameraState | null;
  /** Filled with a reader of the camera now. */
  cameraOut?: { current: (() => CameraState) | null };
  /** The entities across the open graphs (ws009 M2): threads between members, or one merged node each. */
  entities?: Entities;
  /** "Merge entities" (view state only): each entity drawn as one node at its members' centroid. */
  merge?: boolean;
  /** The members "Show across graphs" picked: their threads at emphasis width. */
  across?: ReadonlySet<string> | null;
  /** A graph's colour by alias, for a merged node's ring segments. */
  colorOf?: (alias: string) => string;
  /** Shadow connections (ws009 M1c): the pairs to draw (each with its model's floor) and, in Selected, a hovered
   * node's own top shadows. */
  shadows?: { pairs: readonly Pair[]; hover?: (node: string) => Pair[] } | null;
  spacing?: number;
  layoutMode?: LayoutMode;
  spineKey?: string;
  /** The selected node's drawn edges past the forced labels (the panel's "+n edges"), told whenever it may change. */
  onUnlabelled?: (n: number) => void;
};

/** The camera that frames `ids` (graph keys), in sigma's framed coordinates; null for none. */
/** The camera that frames `ids`, zoomed out at most to `widest` (an agent's camera move keeps 1.2; Fit shows every node). */
function framedBox(sigma: Sigma<NodeAttrs, EdgeAttrs>, graph: ViewGraph, ids: readonly string[], widest = 1.2): CameraState | null {
  if (ids.length === 0) return null;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const id of ids) {
    const { x, y } = graph.getNodeAttributes(id);
    const f = sigma.viewportToFramedGraph(sigma.graphToViewport({ x, y }));
    [x0, y0, x1, y1] = [Math.min(x0, f.x), Math.min(y0, f.y), Math.max(x1, f.x), Math.max(y1, f.y)];
  }
  const span = Math.max(x1 - x0, y1 - y0);
  const ratio = Math.min(Math.max(span * 1.4, 0.15), widest);
  return { x: (x0 + x1) / 2, y: (y0 + y1) / 2, ratio, angle: sigma.getCamera().angle };
}

/** The spine fit's margins in pixels: [left, right, top and bottom]; the right holds the last band's labels. */
const SPINE_PAD = [40, 140, 40] as const;

/** The camera that fits `points` (graph coordinates) inside the viewport less `pad`, measured on screen now. */
function screenBox(sigma: Sigma<NodeAttrs, EdgeAttrs>, points: readonly { x: number; y: number }[], pad: readonly [number, number, number]): CameraState | null {
  if (points.length === 0) return null;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const p of points) {
    const v = sigma.graphToViewport(p);
    [x0, y0, x1, y1] = [Math.min(x0, v.x), Math.min(y0, v.y), Math.max(x1, v.x), Math.max(y1, v.y)];
  }
  const { width, height } = sigma.getDimensions();
  const [l, r, tb] = pad;
  const scale = Math.max((x1 - x0) / Math.max(width - l - r, 1), (y1 - y0) / Math.max(height - 2 * tb, 1), 1e-3);
  const camera = sigma.getCamera();
  // The box's centre, shifted so the extra room on the right falls right of it.
  const c = sigma.viewportToFramedGraph({ x: (x0 + x1) / 2 + ((r - l) / 2) * scale, y: (y0 + y1) / 2 });
  return { x: c.x, y: c.y, ratio: camera.ratio * scale, angle: camera.angle };
}

/** The node panel's width (components/node-panel.tsx w-[420px] max-w-[90vw]). */
const PANEL_W = 420;

const NO_AXES: readonly SpineAxis[] = [];

/** The spine view's lines: a hairline through the anchors' centre, a tick at each band, and an arrowhead at the later
 * end, so the direction of time reads without a legend. */
function drawSpines(ctx: CanvasRenderingContext2D, sigma: Sigma<NodeAttrs, EdgeAttrs>, axes: readonly SpineAxis[], palette: CanvasPalette) {
  for (const axis of axes) {
    const a = sigma.graphToViewport({ x: axis.x0, y: axis.y });
    const b = sigma.graphToViewport({ x: axis.x1, y: axis.y });
    ctx.save();
    ctx.strokeStyle = palette.structure;
    ctx.fillStyle = palette.structure;
    ctx.globalAlpha = 0.55;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
    for (const x of axis.bands) {
      const t = sigma.graphToViewport({ x, y: axis.y });
      ctx.beginPath();
      ctx.moveTo(t.x, t.y - 5);
      ctx.lineTo(t.x, t.y + 5);
      ctx.stroke();
    }
    const angle = Math.atan2(b.y - a.y, b.x - a.x);
    ctx.beginPath();
    ctx.moveTo(b.x, b.y);
    ctx.lineTo(b.x - 9 * Math.cos(angle - 0.4), b.y - 9 * Math.sin(angle - 0.4));
    ctx.lineTo(b.x - 9 * Math.cos(angle + 0.4), b.y - 9 * Math.sin(angle + 0.4));
    ctx.closePath();
    ctx.fill();
    ctx.restore();
  }
}

function drawHover(palette: CanvasPalette) {
  return (
    context: CanvasRenderingContext2D,
    data: PartialButFor<NodeDisplayData, "x" | "y" | "size" | "label" | "color">,
    settings: Settings<NodeAttrs, EdgeAttrs>,
  ) => {
    const size = settings.labelSize + 1;
    context.font = `500 ${size}px ${settings.labelFont}`;
    const label = data.label ?? "";
    const width = Math.min(context.measureText(label).width, 420);
    const pad = 6;
    const x = data.x + data.size + 6;
    const y = data.y;
    const fui = palette.skin === "fui";
    context.save();
    context.shadowColor = fui ? "transparent" : "rgba(0,0,0,0.18)";
    context.shadowBlur = fui ? 0 : 10;
    context.fillStyle = palette.halo;
    context.beginPath();
    context.roundRect(x - pad, y - size / 2 - pad, width + pad * 2, size + pad * 2, fui ? 2 : 7);
    context.fill();
    if (fui) {
      context.strokeStyle = palette.chrome;
      context.lineWidth = 1;
      context.stroke();
    }
    context.restore();
    // The hover ring and the selection halo trace the node's shape (plan 198), 3 px out.
    context.beginPath();
    shapePath(context, shapeAt("shape" in data ? data.shape : undefined), data.x, data.y, data.size + 3);
    context.strokeStyle = palette.accent;
    context.lineWidth = 2;
    context.stroke();
    context.fillStyle = palette.ink;
    context.textBaseline = "middle";
    let text = label;
    if (context.measureText(text).width > 420) {
      while (text.length > 1 && context.measureText(`${text}…`).width > 420) text = text.slice(0, -1);
      text = `${text}…`;
    }
    context.fillText(text, x, y);
  };
}

/** A shift-drag shorter than this, in screen pixels, is a shift-click, not a lasso. */
const LASSO_MIN = 6;

const shiftHeld = (e: { event: MouseCoords }) => (e.event.original as MouseEvent).shiftKey === true;

/** Hull padding around the outermost nodes, in screen pixels. */
const HULL_PAD = 14;
/** Cluster labels show only at or above this camera ratio (zoomed out). */
const LABEL_RATIO = 0.5;

/** The smallest drawn size (px) that gets a node label, by graph size, at today's node size. */
function labelThreshold(order: number): number {
  return order > 5000 ? 6 : order > 1000 ? 7 : 6;
}

const MAX_GROUP_LABELS = 40;

/** Edge-label texts whose widths are held, and label images (about 30 KB each at 2x): relations by chevron side and
 * colour come to tens, so these hold every one in use while bounding a graph with very many relations. */
const LABEL_WIDTHS_HELD = 1024;
const LABEL_IMAGES_HELD = 256;

/** Node labels per 200 px grid cell: a small graph labels every node (review B1), as it labels every edge
 * (SMALL_GRAPH_EDGES); a bigger one keeps main's half a label per cell. */
function labelDensity(order: number): number {
  return order <= SMALL_GRAPH_EDGES ? 100 : 0.5;
}

type Hull = { rank: number; points: number[]; cx: number; cy: number };

/** Hulls in graph coordinates, one per group of HULL_MIN or more shown nodes, biggest first. */
function computeHulls(graph: ViewGraph, groups: GroupInfo): Hull[] {
  const points = new Map<number, number[]>();
  graph.forEachNode((_n, a) => {
    // A merged member is drawn at its entity's centroid, outside its graph's hull (ws009 M2).
    if (a.hidden || a.group < 0 || groups.sizes[a.group] < HULL_MIN || a.homeX !== undefined) return;
    const list = points.get(a.group);
    if (list) list.push(a.x, a.y);
    else points.set(a.group, [a.x, a.y]);
  });
  const out: Hull[] = [];
  for (const [rank, list] of points) {
    if (list.length / 2 < HULL_MIN) continue;
    let cx = 0;
    let cy = 0;
    for (let i = 0; i < list.length; i += 2) {
      cx += list[i];
      cy += list[i + 1];
    }
    out.push({ rank, points: convexHull(list), cx: (cx * 2) / list.length, cy: (cy * 2) / list.length });
  }
  return out.sort((a, b) => a.rank - b.rank);
}

const REDUCED = "(prefers-reduced-motion: reduce)";
function onReducedMotion(changed: () => void) {
  const m = window.matchMedia(REDUCED);
  m.addEventListener("change", changed);
  return () => m.removeEventListener("change", changed);
}

/** The person's reduced-motion setting, live. */
export function useReducedMotion(): boolean {
  return useSyncExternalStore(onReducedMotion, () => window.matchMedia(REDUCED).matches, () => false);
}

/** The FUI grid's cell, in screen pixels: chrome level, under everything. */
const GRID = 48;
const NO_NOTES: ReadonlyMap<string, string> = new Map();
const NO_MODES: RelationModes = new Map();
const NO_SYMMETRIC: ReadonlyMap<string, ReadonlySet<string>> = new Map();
const NO_PLACES: ReadonlyMap<string, Place> = new Map();
/** A thread's width, in screen pixels: confirmed, and picked by "Show across graphs" (ws009 M2 design point 1). */
const THREAD = { normal: 1.2, emphasis: 2.4 };

function Behaviour({ graph, palette, selected, selection, jump, focused, groups, layoutDone, offRelations, relationModes = NO_MODES, symmetric = NO_SYMMETRIC, version, onPick, onClear, onLasso, onJumped, agentMarks, fit, gesture, agentFocusNode = null, agentNotes = NO_NOTES, live = null, readouts = null, panelOpen = false, hold = null, camera: cameraTo = null, cameraOut, entities = NO_ENTITIES, merge = false, across = null, colorOf, shadows = null, spacing = SPACING.default, layoutMode = "normal", spineKey = "", onUnlabelled, spineAxes = NO_AXES }: Props) {
  const sigma = useSigma<NodeAttrs, EdgeAttrs>();
  // Merged places (ws009 M2), recomputed lazily after any node change (layout, visibility, live), read by the reducers
  // and the overlays.
  const places = useRef<{ map: Map<string, Place>; dirty: boolean }>({ map: new Map(), dirty: true });
  useEffect(() => {
    places.current.dirty = true;
    const mark = () => {
      places.current.dirty = true;
    };
    graph.on("eachNodeAttributesUpdated", mark);
    graph.on("nodeAttributesUpdated", mark);
    return () => {
      graph.off("eachNodeAttributesUpdated", mark);
      graph.off("nodeAttributesUpdated", mark);
    };
  }, [graph, entities, merge]);
  const proposedPairs = useMemo(() => new Set(entities.proposed.flatMap(([s, t]) => [`${s}|${t}`, `${t}|${s}`])), [entities]);
  const placesNow = useCallback((): ReadonlyMap<string, Place> => {
    if (!merge) return NO_PLACES;
    if (places.current.dirty) places.current = { map: mergedPlaces(graph, entities), dirty: false };
    return places.current.map;
  }, [graph, entities, merge]);
  /** A node's centre and radius on screen, where it is drawn (merged or not). */
  const onScreen = useCallback(
    (id: string) => {
      const p = placesNow().get(id);
      const a = graph.getNodeAttributes(id);
      return { ...sigma.graphToViewport({ x: p?.x ?? a.x, y: p?.y ?? a.y }), r: sigma.scaleSize(sigma.getNodeDisplayData(id)?.size ?? (p?.size ?? a.size)) };
    },
    [graph, sigma, placesNow],
  );
  const reduced = useReducedMotion();
  const fui = palette.skin === "fui";
  // Camera eases (400 to 600 ms) are instant with reduced motion: 1 ms, since sigma divides by the duration.
  const reducedNow = useRef(reduced);
  useEffect(() => {
    reducedNow.current = reduced;
  }, [reduced]);
  const ease = useRef((ms: number) => (reducedNow.current ? 1 : ms)).current;
  const registerEvents = useRegisterEvents<NodeAttrs, EdgeAttrs>();
  const setSettings = useSetSettings<NodeAttrs, EdgeAttrs>();
  // A lasso ends on mouseup, and the browser then sends a click, which must not select or clear anything.
  const afterLasso = useRef(false);
  // The hovered node, for its edges' labels: read by the label drawer, which sigma reruns on hover without the
  // reducers (plan 150).
  const hovered = useRef<string | null>(null);
  // Each parallel or opposite edge's curvature, kept up to date as live changes add and drop edges; `edgesChanged`
  // counts those changes, so the label drawer knows its forced set is stale.
  const curves = useRef<Map<string, number>>(new Map());
  const edgesChanged = useRef(0);
  const nodeLabelBoxes = useRef<LabelBox[]>([]);

  useEffect(() => {
    const lassoEnded = () => {
      const was = afterLasso.current;
      afterLasso.current = false;
      return was;
    };
    registerEvents({
      clickNode: (e) => {
        if (!lassoEnded()) onPick(e.node, shiftHeld(e));
      },
      clickStage: (e) => {
        if (!lassoEnded() && !shiftHeld(e)) onClear();
      },
      // sigma makes a second click within 300 ms a double-click (zoom) instead of a click. Quick shift-clicks are
      // each a toggle, and the click that ends a lasso is never a zoom.
      doubleClickNode: (e) => {
        if (lassoEnded()) e.preventSigmaDefault();
        else if (shiftHeld(e)) {
          e.preventSigmaDefault();
          onPick(e.node, true);
        }
      },
      doubleClickStage: (e) => {
        if (lassoEnded() || shiftHeld(e)) e.preventSigmaDefault();
      },
      // Hover changes no reducer: sigma's hover layer draws the label (drawHover), so a hover never reprocesses
      // the whole graph, which at 10k nodes cost frames while panning.
      enterNode: (e) => {
        hovered.current = e.node;
        sigma.getContainer().style.cursor = "pointer";
      },
      leaveNode: () => {
        hovered.current = null;
        sigma.getContainer().style.cursor = "";
      },
    });
  }, [registerEvents, onPick, onClear, sigma]);

  // The lasso: shift and the left button down, then drag. While it is drawn the camera does not pan (the move's sigma
  // default is prevented). On release it adds the shown nodes whose centre is inside; hidden nodes never.
  useEffect(() => {
    if (!sigma.getCanvases().lasso) {
      sigma.createCanvasContext("lasso", { beforeLayer: "mouse" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const ctx = sigma.getCanvases().lasso.getContext("2d")!;
    let lasso: { points: number[]; moved: boolean } | null = null;
    const wipe = () => {
      const { width, height } = sigma.getDimensions();
      ctx.clearRect(0, 0, width, height);
    };
    const draw = (points: number[]) => {
      wipe();
      ctx.beginPath();
      for (let i = 0; i < points.length; i += 2) {
        if (i === 0) ctx.moveTo(points[i], points[i + 1]);
        else ctx.lineTo(points[i], points[i + 1]);
      }
      ctx.closePath();
      ctx.globalAlpha = 0.1;
      ctx.fillStyle = palette.accent;
      ctx.fill();
      ctx.globalAlpha = 1;
      ctx.setLineDash([5, 4]);
      ctx.lineWidth = 1.5;
      ctx.strokeStyle = palette.accent;
      ctx.stroke();
      ctx.setLineDash([]);
    };
    const down = (e: { event: MouseCoords }) => {
      afterLasso.current = false;
      const m = e.event.original as MouseEvent;
      lasso = m.shiftKey === true && m.button === 0 ? { points: [e.event.x, e.event.y], moved: false } : null;
    };
    const move = (e: { event: MouseCoords; preventSigmaDefault: () => void }) => {
      if (!lasso) return;
      e.preventSigmaDefault();
      const p = lasso.points;
      if (Math.hypot(e.event.x - p[p.length - 2], e.event.y - p[p.length - 1]) < 3) return;
      p.push(e.event.x, e.event.y);
      lasso.moved ||= Math.hypot(e.event.x - p[0], e.event.y - p[1]) >= LASSO_MIN;
      if (lasso.moved) draw(p);
    };
    const up = () => {
      const done = lasso;
      lasso = null;
      if (!done) return;
      wipe();
      if (!done.moved) return;
      afterLasso.current = true;
      const nodes: { id: string; x: number; y: number; hidden: boolean }[] = [];
      graph.forEachNode((id, a) => {
        const v = sigma.graphToViewport({ x: a.x, y: a.y });
        nodes.push({ id, x: v.x, y: v.y, hidden: a.hidden === true });
      });
      onLasso(nodesInLasso(done.points, nodes));
    };
    sigma.on("downStage", down);
    sigma.on("downNode", down);
    sigma.on("moveBody", move);
    sigma.on("upStage", up);
    sigma.on("upNode", up);
    return () => {
      sigma.off("downStage", down);
      sigma.off("downNode", down);
      sigma.off("moveBody", move);
      sigma.off("upStage", up);
      sigma.off("upNode", up);
      wipe();
    };
  }, [sigma, graph, palette, onLasso]);

  const spacingFrame = useRef<{ graph: ViewGraph; spacing: number; mode: LayoutMode; spine: string; box: Extent | null }>({ graph, spacing, mode: layoutMode, spine: spineKey, box: null });
  // Keep the frame while spacing changes: automatic normalization would otherwise cancel the increased distances.
  useEffect(() => {
    const frame = spacingFrame.current;
    if (frame.graph !== graph || frame.mode !== layoutMode || frame.spine !== spineKey) frame.box = null;
    else if (frame.spacing !== spacing && !hold) frame.box ??= graphExtent(graph);
    [frame.graph, frame.spacing, frame.mode, frame.spine] = [graph, spacing, layoutMode, spineKey];
    if (!hold && !frame.box && !cameraTo && sigma.getCustomBBox() === null) return;
    sigma.setCustomBBox(hold ?? frame.box);
    if (cameraTo) sigma.getCamera().setState(cameraTo);
    sigma.refresh();
  }, [sigma, graph, hold, cameraTo, spacing, layoutMode, spineKey]);
  useEffect(() => {
    if (!cameraOut) return;
    const read = () => sigma.getCamera().getState();
    cameraOut.current = read;
    return () => {
      if (cameraOut.current === read) cameraOut.current = null;
    };
  }, [sigma, cameraOut]);

  // The whole graph: sigma's reset, or with a held frame the shown nodes' own box (a reset would show the old frame).
  const fitAll = useRef<(duration: number) => void>(() => {});
  useEffect(() => {
    fitAll.current = (duration: number) => {
      sigma.refresh();
      const points = [...graph.filterNodes((_n, a) => !a.hidden).map((id) => graph.getNodeAttributes(id)), ...spineAxes.flatMap((a) => [{ x: a.x0, y: a.y }, { x: a.x1, y: a.y }])];
      const target = screenBox(sigma, points, spineAxes.length > 0 ? SPINE_PAD : [40, 40, 40]);
      if (target) void sigma.getCamera().animate(target, { duration });
    };
  }, [sigma, graph, hold, spineAxes]);

  useEffect(() => {
    if (layoutDone && !hold && !spacingFrame.current.box) sigma.getCamera().animatedReset({ duration: ease(400) });
  }, [layoutDone, sigma, ease, hold]);

  // Where three nodes are, on screen (@) and in the layout (~), for checks that an agent's open moved nothing
  // (data-probe): the three smallest keys, so the same nodes whatever order graphs joined in.
  useEffect(() => {
    const el = sigma.getContainer();
    const picked = graph.nodes().sort().slice(0, 3);
    const probe = () => {
      el.dataset.probe = picked
        .filter((n) => graph.hasNode(n))
        .map((n) => {
          const a = graph.getNodeAttributes(n);
          const v = sigma.graphToViewport({ x: a.x, y: a.y });
          return `${n}@${Math.round(v.x)},${Math.round(v.y)}~${a.x.toFixed(2)},${a.y.toFixed(2)}`;
        })
        .join(" ");
    };
    sigma.on("afterRender", probe);
    return () => {
      sigma.off("afterRender", probe);
    };
  }, [sigma, graph]);

  // The whole gesture, not only its start (review R224-4). Capture phase, because sigma's own handlers stop these
  // events from bubbling; moves, releases and cancels are watched on the window, since a drag can end anywhere.
  useEffect(() => {
    if (!gesture) return;
    const el = sigma.getContainer();
    const now = () => Date.now();
    const opts = { capture: true, passive: true } as const;
    const press = (e: PointerEvent) => gesture.press(e.pointerId, now());
    const move = () => gesture.move(now());
    const release = (e: PointerEvent) => gesture.release(e.pointerId, now());
    const wheel = () => gesture.wheel(now());
    const blur = () => gesture.releaseAll(now());
    el.addEventListener("pointerdown", press, opts);
    el.addEventListener("wheel", wheel, opts);
    window.addEventListener("pointermove", move, opts);
    window.addEventListener("pointerup", release, opts);
    window.addEventListener("pointercancel", release, opts);
    window.addEventListener("blur", blur);
    return () => {
      el.removeEventListener("pointerdown", press, opts);
      el.removeEventListener("wheel", wheel, opts);
      window.removeEventListener("pointermove", move, opts);
      window.removeEventListener("pointerup", release, opts);
      window.removeEventListener("pointercancel", release, opts);
      window.removeEventListener("blur", blur);
    };
  }, [sigma, gesture]);

  // The agent's badge: a small disc on each highlighted node that is shown, drawn above the nodes and their labels after
  // each render, and under the FUI callouts (ui-expert K1-N1): the callout layer, made later, goes above it.
  useEffect(() => {
    if (!sigma.getCanvases().agentBadges) {
      sigma.createCanvasContext("agentBadges", { beforeLayer: sigma.getCanvases().callouts ? "callouts" : "hovers" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const ctx = sigma.getCanvases().agentBadges.getContext("2d")!;
    const draw = () => {
      const { width, height } = sigma.getDimensions();
      ctx.clearRect(0, 0, width, height);
      for (const id of agentMarks) {
        if (!graph.hasNode(id) || graph.getNodeAttribute(id, "hidden")) continue;
        const { x, y, r } = onScreen(id);
        const p = { x, y };
        const bx = p.x + r * 0.8 + 3;
        const by = p.y - r * 0.8 - 3;
        ctx.beginPath();
        ctx.arc(bx, by, 6, 0, Math.PI * 2);
        ctx.fillStyle = palette.agent;
        ctx.fill();
        ctx.lineWidth = 1.5;
        ctx.strokeStyle = palette.halo;
        ctx.stroke();
        ctx.fillStyle = palette.halo;
        ctx.font = `700 8px ${palette.font}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText("A", bx, by + 0.5);
      }
    };
    sigma.on("afterRender", draw);
    sigma.refresh();
    return () => {
      sigma.off("afterRender", draw);
    };
  }, [sigma, graph, agentMarks, palette, onScreen]);

  // An agent's fit: the shown nodes it named, or the whole graph.
  const fitted = useRef(0);
  useEffect(() => {
    if (!fit || fit.n === fitted.current || !layoutDone) return;
    fitted.current = fit.n;
    const camera = sigma.getCamera();
    const decision = canvasFit(fit.nodes, (id) => graph.hasNode(id) && !graph.getNodeAttribute(id, "hidden"));
    if (decision === "reset") {
      fitAll.current(ease(600));
      return;
    }
    if (decision === "stay") return;
    const target = framedBox(sigma, graph, decision);
    if (target) void camera.animate(target, { duration: ease(600) });
  }, [fit, graph, sigma, layoutDone, ease]);

  // Hulls on a 2D canvas under sigma's WebGL edges, cluster labels above the nodes. Both redraw after each sigma
  // render, which sigma already batches to one per animation frame, so camera moves cost one redraw a frame.
  useEffect(() => {
    if (!sigma.getCanvases().hulls) {
      sigma.createCanvasContext("hulls", { beforeLayer: "edges" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.createCanvasContext("groupLabels", { beforeLayer: "labels" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const hullCtx = sigma.getCanvases().hulls.getContext("2d")!;
    const labelCtx = sigma.getCanvases().groupLabels.getContext("2d")!;
    let hulls: Hull[] = [];
    let dirty = true;
    const markDirty = () => {
      dirty = true;
    };
    graph.on("eachNodeAttributesUpdated", markDirty);
    graph.on("nodeAttributesUpdated", markDirty);
    const fills = new Map<number, string>();
    const draw = () => {
      const { width, height } = sigma.getDimensions();
      hullCtx.clearRect(0, 0, width, height);
      labelCtx.clearRect(0, 0, width, height);
      if (palette.skin === "fui") {
        hullCtx.beginPath();
        for (let x = GRID; x < width; x += GRID) {
          hullCtx.moveTo(x + 0.5, 0);
          hullCtx.lineTo(x + 0.5, height);
        }
        for (let y = GRID; y < height; y += GRID) {
          hullCtx.moveTo(0, y + 0.5);
          hullCtx.lineTo(width, y + 0.5);
        }
        hullCtx.lineWidth = 1;
        hullCtx.strokeStyle = palette.grid;
        hullCtx.stroke();
      }
      drawSpines(hullCtx, sigma, spineAxes, palette);
      if (!groups) return;
      if (dirty) {
        hulls = computeHulls(graph, groups);
        dirty = false;
      }
      hullCtx.lineJoin = "round";
      hullCtx.lineCap = "round";
      hullCtx.lineWidth = HULL_PAD * 2;
      for (const hull of hulls) {
        let fill = fills.get(hull.rank);
        if (!fill) {
          fill = mix(palette.canvas, groups.colors?.[hull.rank] ?? groupColor(hull.rank, groups.sizes[hull.rank], palette.theme), HULL_TINT[palette.theme]);
          fills.set(hull.rank, fill);
        }
        hullCtx.beginPath();
        for (let i = 0; i < hull.points.length; i += 2) {
          const p = sigma.graphToViewport({ x: hull.points[i], y: hull.points[i + 1] });
          if (i === 0) hullCtx.moveTo(p.x, p.y);
          else hullCtx.lineTo(p.x, p.y);
        }
        hullCtx.closePath();
        hullCtx.fillStyle = fill;
        hullCtx.strokeStyle = fill;
        hullCtx.fill();
        hullCtx.stroke();
      }
      if (sigma.getCamera().ratio < LABEL_RATIO) return;
      // A pill per group: the head node's title and the group's size. The head's own node label is skipped
      // meanwhile (drawLabel below), so the title is not on screen twice.
      labelCtx.font = `600 12px ${palette.font}`;
      labelCtx.textAlign = "center";
      labelCtx.textBaseline = "middle";
      labelCtx.lineWidth = 1;
      const pills = hulls.slice(0, MAX_GROUP_LABELS).map((hull) => {
        let title = groups.labels[hull.rank] ?? "";
        if (title.length > 30) title = `${title.slice(0, 29)}…`;
        // A held graph's pill counts its changes not applied yet (plan 152): "+12", or "+many".
        const held = groups.held?.[hull.rank];
        const text = `${title} · ${groups.sizes[hull.rank]}${held ? ` +${held}` : ""}`;
        const p = sigma.graphToViewport({ x: hull.cx, y: hull.cy });
        const w = labelCtx.measureText(text).width + 16;
        const box: Box = [p.x - w / 2, p.y - 11, p.x + w / 2, p.y + 11];
        return { hull, text, p, w, box };
      });
      // Biggest group first (hulls are by rank); a pill touching one already drawn is skipped (N10). In the FUI skins
      // at most FUI_PILLS are drawn, quietly (K1-N2).
      const keep = placePills(pills.map((pill) => pill.box));
      let drawn = 0;
      pills.forEach(({ hull, text, p, w, box }, i) => {
        if (!keep[i]) return;
        if (box[0] < 0 || box[1] < 0 || box[2] > width || box[3] > height || nodeLabelBoxes.current.some((b) => overlaps(b, { x0: box[0], y0: box[1], x1: box[2], y1: box[3] }))) return;
        const look = pillLook(palette.skin === "fui", palette, groups.colors?.[hull.rank] ?? groupColor(hull.rank, groups.sizes[hull.rank], palette.theme), MAX_GROUP_LABELS);
        if (drawn >= look.cap) return;
        drawn++;
        labelCtx.beginPath();
        labelCtx.roundRect(box[0], box[1], w, 22, palette.skin === "fui" ? 2 : 11);
        labelCtx.fillStyle = palette.halo;
        labelCtx.strokeStyle = look.border;
        labelCtx.fill();
        labelCtx.stroke();
        labelCtx.fillStyle = look.text;
        labelCtx.fillText(text, p.x, p.y + 0.5);
      });
    };
    sigma.on("afterRender", draw);
    sigma.refresh();
    return () => {
      sigma.off("afterRender", draw);
      graph.off("eachNodeAttributesUpdated", markDirty);
      graph.off("nodeAttributesUpdated", markDirty);
    };
  }, [sigma, graph, groups, palette, spineAxes]);

  // Curves for parallel and opposite pairs (plan 150). Sigma reprocesses only an added edge, so after a live change
  // every edge whose curvature moved is refreshed too: a pair that gains an edge bends, one that loses it straightens.
  useEffect(() => {
    curves.current = curvatures(graph);
    let queued = false;
    const changed = () => {
      edgesChanged.current++;
      if (queued) return;
      queued = true;
      queueMicrotask(() => {
        queued = false;
        const before = curves.current;
        const after = curvatures(graph);
        curves.current = after;
        const moved = [...new Set([...before.keys(), ...after.keys()])].filter((e) => graph.hasEdge(e) && before.get(e) !== after.get(e));
        if (moved.length > 0) sigma.refresh({ partialGraph: { edges: moved }, schedule: true });
      });
    };
    graph.on("edgeAdded", changed);
    graph.on("edgeDropped", changed);
    graph.on("cleared", changed);
    graph.on("edgesCleared", changed);
    return () => {
      graph.off("edgeAdded", changed);
      graph.off("edgeDropped", changed);
      graph.off("cleared", changed);
      graph.off("edgesCleared", changed);
    };
  }, [graph, sigma]);

  // Selection, hover and focus. Focus dims everything outside the neighbourhood through the reducers (the graph is
  // never rebuilt) and colours the neighbourhood's edges by relation. It never uses a halo, so the agent highlight
  // (design note section 5) keeps its own channel.
  useEffect(() => {
    const near = new Set<string>();
    if (selected && graph.hasNode(selected)) {
      graph.forEachEdge(selected, (_e, a, s, t) => {
        if (!offRelations.has(a.kind)) near.add(s === selected ? t : s);
      });
    }
    const heads = new Set(groups?.heads ?? []);
    // In the FUI skins the selection and the agent's nodes are named by their callouts, not by sigma's labels.
    const called = palette.skin === "fui" ? new Set([...selection, ...agentMarks, ...(agentFocusNode ? [agentFocusNode] : [])]) : null;
    const instrument = palette.theme === "instrument";
    const mode = (kind: string) => relationMode(kind, relationModes, offRelations);
    // An edge inside the focus (any edge with no focus): coloured by relation and labelled; the rest are dimmed and
    // unlabelled. The edge reducer and the label pass both ask this.
    const inFocus = (s: string, t: string, kind: string) => !focused || (focused.has(s) && focused.has(t) && kind !== SIMILAR_TO);
    let centre: string | null = null;
    focused?.forEach((d, n) => {
      if (d === 0) centre = n;
    });
    const at = { selected, hovered: null as string | null, focusCentre: centre };
    // What the edge reducer leaves out: a relation turned off, an edge inside one merged entity (it has no length left),
    // a denial, and an identity edge a thread already draws.
    const leftOut = edgeLeftOut(graph, {
      off: offRelations,
      together: (s, t) => {
        if (!merge) return false;
        const [ps, pt] = [placesNow().get(s), placesNow().get(t)];
        return !!ps && !!pt && ps.entity === pt.entity;
      },
      entities,
      proposedPairs,
    });
    // Only edges the reducer draws get forced labels or count in the panel's "+n edges" (reviewer R370-2).
    const shown = forcedShown(graph, offRelations, focused, leftOut);
    onUnlabelled?.(selected && graph.hasNode(selected) ? forcedLabels(graph, { ...at, focusCentre: null }, shown).more : 0);
    // The forced set with the hovered node's edges in their place (after the selected node's, before focus).
    let forcedFor: { hovered: string | null; tick: number; edges: Set<string> } | null = null;
    const forcedNow = () => {
      if (!forcedFor || forcedFor.hovered !== hovered.current || forcedFor.tick !== edgesChanged.current) {
        const edges = forcedLabels(graph, { ...at, hovered: hovered.current }, shown).edges;
        forcedFor = { hovered: hovered.current, tick: edgesChanged.current, edges };
      }
      return forcedFor.edges;
    };
    // Each node's drawn incoming edges, for the heads into a hub (E1-N1), counted again after a live change.
    let hubFor = { tick: edgesChanged.current, counts: incomingDrawn(graph, leftOut) };
    const incomingNow = (node: string) => {
      if (hubFor.tick !== edgesChanged.current) hubFor = { tick: edgesChanged.current, counts: incomingDrawn(graph, leftOut) };
      return hubFor.counts.get(node) ?? 0;
    };
    const fuiLabels = palette.skin === "fui";
    const labelFont = fuiLabels ? palette.mono : palette.font;
    const ruleColor = fuiLabels ? palette.chromeText : palette.structure;
    const forcedColor = fuiLabels ? palette.ink : palette.structure;
    // The node labels drawn this frame (sigma draws them before the edge labels): a zoom-rule edge label that would
    // cross one is skipped, so node labels win at 10k (ui-expert E1-N3). A hover's edge-label pass alone keeps
    // the last frame's boxes, as the node labels have not moved.
    let nodeLabels: LabelBox[] = [];
    // The spine view (no label soup at 1,000 nodes): each anchor's label is held first, cut to the room before the next
    // anchor; any other label shows only once rows are a label apart on screen, and never over one already drawn.
    const spineAnchors = new Set(spineAxes.flatMap((a) => a.anchors));
    let anchorText = new Map<string, string>();
    let anchorBoxes: LabelBox[] = [];
    let rowsRoom = false;
    let anchorsDue = false;
    let hubsDue = false;
    const hubLabels = new Map<string, { text: string; box: LabelBox }>();
    // Sigma adds 3px after this radius; leave 8px past the actual shape, including enlarged hubs.
    const shapeSize = (data: { size: number; shape?: unknown }) => data.size * extentOf(shapeAt(data.shape)).halfWidth + 5;
    const newFrame = () => {
      nodeLabels = [];
      nodeLabelBoxes.current = nodeLabels;
      anchorsDue = spineAxes.length > 0;
      hubsDue = spineAxes.length === 0;
      hubLabels.clear();
    };
    const holdHubs = () => {
      hubsDue = false;
      const { labelSize, labelWeight, labelFont } = sigma.getSettings();
      const ctx = sigma.getCanvases().labels.getContext("2d")!;
      ctx.font = `${labelWeight} ${labelSize}px ${labelFont}`;
      const { width } = sigma.getDimensions();
      const shown = graph.filterNodes((_k, a) => a.hub === true && !a.hidden).map((key) => {
        const d = sigma.getNodeDisplayData(key)!;
        const v = sigma.framedGraphToViewport(d);
        return { key, ...v, size: shapeSize({ ...d, size: sigma.scaleSize(d.size) }), label: d.label };
      }).filter((a) => a.label && !called?.has(a.key));
      for (const a of shown) {
        let room = Math.min(180, width - a.x - a.size - 11);
        for (const b of shown) if (b.x > a.x && Math.abs(b.y - a.y) < labelSize * 1.1) room = Math.min(room, b.x - b.size - a.x - a.size - 11);
        const text = ctx.measureText(a.label!).width <= room ? a.label! : fitCallout(a.label!, room, (t) => ctx.measureText(t).width, 400);
        hubLabels.set(a.key, { text, box: nodeLabelBox(a.x, a.y, a.size, labelSize, ctx.measureText(text).width) });
      }
    };
    // Each anchor's label width, and its cut text by width: measured once per label, not every frame.
    const labelWidths = new Map<string, number>();
    const cuts = new Map<string, { text: string; width: number }>();
    // The widths anchorLabels gave, kept while the zoom (and so the anchors' spacing on screen) stays; a pan keeps them.
    let widthsFor: { ratio: number; angle: number; widths: Map<string, number> } | null = null;
    // Run at the frame's first label, once sigma has the frame's node data.
    const holdAnchors = () => {
      anchorsDue = false;
      const { labelSize, labelWeight, labelFont: font } = sigma.getSettings();
      const pitch = Math.min(...spineAxes.map((a) => Math.abs(sigma.graphToViewport({ x: a.x0, y: a.y }).y - sigma.graphToViewport({ x: a.x0, y: a.y + a.pitch }).y)));
      rowsRoom = rowsLabelled(pitch, labelSize);
      const ctx = sigma.getCanvases().labels.getContext("2d")!;
      ctx.font = `${labelWeight} ${labelSize}px ${font}`;
      const measure = (t: string) => {
        let w = labelWidths.get(t);
        if (w === undefined) labelWidths.set(t, (w = ctx.measureText(t).width));
        return w;
      };
      const shown = [...spineAnchors].filter((k) => graph.hasNode(k) && !graph.getNodeAttribute(k, "hidden")).map((key) => {
        const d = sigma.getNodeDisplayData(key)!;
        const v = sigma.framedGraphToViewport(d);
        const label = d.label ?? "";
        return { key, x: v.x, y: v.y, size: shapeSize({ size: sigma.scaleSize(d.size), shape: (d as { shape?: unknown }).shape }), label, width: label === "" ? 0 : measure(label) };
      }).filter((a) => a.label !== "");
      const { ratio, angle } = sigma.getCamera().getState();
      if (!widthsFor || widthsFor.ratio !== ratio || widthsFor.angle !== angle) widthsFor = { ratio, angle, widths: anchorLabels(shown, labelSize) };
      [anchorText, anchorBoxes] = [new Map(), []];
      for (const a of shown) {
        const width = widthsFor.widths.get(a.key);
        if (width === undefined) continue;
        const id = `${a.label}\u0000${width}`;
        let cut = cuts.get(id);
        if (!cut) {
          // Every zoom gives new widths: start over rather than keep each one.
          if (cuts.size > 4 * spineAnchors.size + 64) cuts.clear();
          const text = width >= a.width ? a.label : fitCallout(a.label, width, (t) => ctx.measureText(t).width, 400);
          cuts.set(id, (cut = { text, width: measure(text) }));
        }
        anchorText.set(a.key, cut.text);
        anchorBoxes.push(nodeLabelBox(a.x, a.y, a.size, labelSize, cut.width));
      }
    };
    sigma.on("beforeRender", newFrame);
    const meetsNodeLabel = (box: LabelBox) => nodeLabels.some((b) => overlaps(b, box));
    // Edge labels are this effect's own pass (plan 198, the UI expert's design note, section 2), on sigma's
    // edge-label layer after each render and after a hover: sigma's pass only offers edges whose two end labels are
    // drawn, and the zoom rule labels any edge long enough on screen. Each drawn edge (the reducer's own predicate,
    // edgeLeftOut, and both ends shown) is listed once per live change.
    let drawnFor: { tick: number; list: { edge: string; s: string; t: string; kind: string }[] } | null = null;
    const drawnNow = () => {
      if (!drawnFor || drawnFor.tick !== edgesChanged.current) {
        const list: { edge: string; s: string; t: string; kind: string }[] = [];
        graph.forEachEdge((edge, a, s, t) => {
          if (!leftOut(edge) && !graph.getNodeAttribute(s, "hidden") && !graph.getNodeAttribute(t, "hidden")) list.push({ edge, s, t, kind: a.kind });
        });
        drawnFor = { tick: edgesChanged.current, list };
      }
      return drawnFor.list;
    };
    const labelCtx = sigma.getCanvases().edgeLabels.getContext("2d")!;
    const widthOf = (kind: string) => textWidth(`${kind} ›`);
    // Both caches belong to this run of the effect, so a palette or font change starts them empty; each is bounded
    // (reviewer R370-3), and a label dropped from one is measured or drawn again on its next frame.
    const texts = new Lru<string, number>(LABEL_WIDTHS_HELD);
    const textWidth = (text: string) =>
      texts.getOr(text, () => {
        labelCtx.font = `500 ${LABEL_PX}px ${labelFont}`;
        return labelCtx.measureText(text).width;
      });
    // Each label text and colour is drawn once, halo and all, into an image the frames then stamp: stroking text is
    // the costly part of a label, and a selected hub has 150 of them every frame.
    const sprites = new Lru<string, { image: HTMLCanvasElement; w: number; h: number }>(LABEL_IMAGES_HELD);
    const stamp = (text: string, style: LabelStyle) => {
      const sprite = sprites.getOr(`${text}\u0000${style.color}`, () => {
        const ratio = window.devicePixelRatio || 1;
        const [w, h] = [Math.ceil(textWidth(text)) + 8, LABEL_PX + 8];
        const image = document.createElement("canvas");
        [image.width, image.height] = [Math.ceil(w * ratio), Math.ceil(h * ratio)];
        const c = image.getContext("2d")!;
        c.scale(ratio, ratio);
        c.font = `500 ${LABEL_PX}px ${style.font}`;
        c.textAlign = "center";
        c.textBaseline = "middle";
        c.lineJoin = "round";
        c.lineWidth = 3;
        c.strokeStyle = style.halo;
        c.strokeText(text, w / 2, h / 2);
        c.fillStyle = style.color;
        c.fillText(text, w / 2, h / 2);
        return { image, w, h };
      });
      labelCtx.drawImage(sprite.image, -sprite.w / 2, -sprite.h / 2, sprite.w, sprite.h);
    };
    const drawEdgeLabels = () => {
      const { width, height } = sigma.getDimensions();
      labelCtx.clearRect(0, 0, width, height);
      // Graph to screen is affine: three points give it, so the hot loop allocates nothing.
      const o = sigma.graphToViewport({ x: 0, y: 0 });
      const ux = sigma.graphToViewport({ x: 1, y: 0 });
      const uy = sigma.graphToViewport({ x: 0, y: 1 });
      const [ax, ay, bx, by] = [ux.x - o.x, ux.y - o.y, uy.x - o.x, uy.y - o.y];
      const scale = sigma.scaleSize(1);
      const places = placesNow();
      const end = (id: string) => places.get(id) ?? graph.getNodeAttributes(id);
      const paint = (edge: string, s: string, t: string, kind: string, how: "forced" | "every" | "rule") => {
        const [ps, pt] = [end(s), end(t)];
        const [sx, sy, tx, ty] = [o.x + ax * ps.x + bx * ps.y, o.y + ay * ps.x + by * ps.y, o.x + ax * pt.x + bx * pt.y, o.y + ay * pt.x + by * pt.y];
        if ((sx < 0 && tx < 0) || (sy < 0 && ty < 0) || (sx > width && tx > width) || (sy > height && ty > height)) return false;
        const [rs, rt] = [sigma.getNodeDisplayData(s)?.size ?? ps.size, sigma.getNodeDisplayData(t)?.size ?? pt.size].map((r) => r * scale);
        if (how === "rule" && (mode(kind) !== "normal" || !fitsZoomRule(Math.hypot(tx - sx, ty - sy), widthOf(kind), rs, rt))) return false;
        // Labelled only where the edge reducer gives it its label: inside the focus, when there is one.
        if (!inFocus(s, t, kind)) return false;
        const curvature = curves.current.get(edge) ?? 0;
        const mid = labelAnchor({ x: sx, y: sy }, { x: tx, y: ty }, curvature);
        if (mid.x < 0 || mid.y < 0 || mid.x > width || mid.y > height) return false;
        const style = { font: labelFont, color: how === "forced" ? forcedColor : ruleColor, halo: palette.canvas };
        return paintEdgeLabel(labelCtx, kind, { x: sx, y: sy, size: rs }, { x: tx, y: ty, size: rt }, curvature, style, how === "rule", meetsNodeLabel, textWidth, stamp);
      };
      const drawn = drawnNow();
      const automatic = drawn.length <= SMALL_GRAPH_EDGES || sigma.getCamera().ratio < LABEL_RATIO ? drawn : [];
      const drew = labelFrame(forcedNow(), automatic, {
        forced: (edge) => {
          if (!graph.hasEdge(edge) || !shown(edge)) return false;
          const [s, t] = graph.extremities(edge);
          return paint(edge, s, t, graph.getEdgeAttribute(edge, "kind"), "forced");
        },
        other: (item, every) => paint(item.edge, item.s, item.t, item.kind, every ? "every" : "rule"),
      });
      // Numbers only, for checks that read what this frame labelled: forced, then the rest.
      const counts = `${drew.forced} ${drew.other}`;
      if (labelCtx.canvas.dataset.labels !== counts) labelCtx.canvas.dataset.labels = counts;
      // And how many node titles sigma drew this frame (the cluster pills name the group heads).
      if (labelCtx.canvas.dataset.nodeLabels !== String(nodeLabels.length)) labelCtx.canvas.dataset.nodeLabels = String(nodeLabels.length);
    };
    sigma.on("afterRender", drawEdgeLabels);
    // A hover redraws only sigma's hover layer: the labels follow on the next frame, with the hovered node's edges.
    let hoverFrame = 0;
    const hoverLabels = () => {
      cancelAnimationFrame(hoverFrame);
      hoverFrame = requestAnimationFrame(drawEdgeLabels);
    };
    sigma.on("enterNode", hoverLabels);
    sigma.on("leaveNode", hoverLabels);
    const sizeProjection = sizeRefresh(() => [sigma.getGraphToViewportRatio(), sigma.scaleSize(1)], () => sigma.scheduleRefresh());
    setSettings({
      defaultDrawNodeLabel: (context, data, settings) => {
        const key = (data as { key?: string }).key ?? "";
        if (called?.has(key)) return;
        const zoomedOut = sigma.getCamera().ratio >= LABEL_RATIO;
        if (zoomedOut && !data.forceLabel && heads.has(key)) return;
        // Beside the shape's right edge (a wide shape reaches past the node's radius).
        const at = { ...data, size: shapeSize(data) };
        if (hubsDue) holdHubs();
        const hub = hubLabels.get(key);
        if (hub) at.label = hub.text;
        if (anchorsDue) holdAnchors();
        if (spineAxes.length > 0 && key !== selected) {
          context.font = `${settings.labelWeight} ${settings.labelSize}px ${settings.labelFont}`;
          if (spineAnchors.has(key)) {
            const text = anchorText.get(key);
            if (text === undefined) return;
            at.label = text;
          } else {
            // A forced label (a neighbour of the selection, an agent's mark) skips the row rule, not the overlap test.
            if ((!data.forceLabel && !rowsRoom) || !data.label) return;
            const box = nodeLabelBox(at.x, at.y, at.size, settings.labelSize, context.measureText(data.label).width);
            if (anchorBoxes.some((b) => overlaps(b, box)) || nodeLabels.some((b) => overlaps(b, box))) return;
          }
        }
        if (at.label && key !== selected) {
          context.font = `${settings.labelWeight} ${settings.labelSize}px ${settings.labelFont}`;
          const box = nodeLabelBox(at.x, at.y, at.size, settings.labelSize, context.measureText(at.label).width);
          const { width, height } = sigma.getDimensions();
          if (box.x0 < 0 || box.y0 < 0 || box.x1 > width || box.y1 > height) return;
          if (!hub && [...hubLabels.values()].some((h) => overlaps(h.box, box))) return;
          if (nodeLabels.some((b) => overlaps(b, box))) return;
        }
        drawDiscNodeLabel(context, at, settings);
        if (at.label) nodeLabels.push(nodeLabelBox(at.x, at.y, at.size, settings.labelSize, context.measureText(at.label).width));
      },
      // Which nodes get labels goes by their nominal size, as on main, not by the size the Spacing slider draws.
      labelRenderedSizeThreshold: labelThreshold(graph.order),
      labelDensity: labelDensity(graph.order),
      labelColor: { color: palette.ink },
      labelFont: palette.font,
      defaultDrawNodeHover: drawHover(palette),
      nodeReducer: (node, data) => {
        const out: Partial<NodeDisplayData> & Record<string, unknown> = { ...data };
        switch (emphasis(node, selected, near, focused)) {
          case "dim":
            out.color = palette.dim;
            out.borderColor = palette.dim;
            out.label = null;
            out.zIndex = 0;
            break;
          case "selected":
            // A highlighted node is drawn on the hover layer with its label; in the FUI skins its callout names it.
            out.highlighted = palette.skin !== "fui";
            out.forceLabel = true;
            out.zIndex = 3;
            out.borderColor = palette.accent;
            out.borderSize = 0.32;
            break;
          case "label":
            out.forceLabel = true;
            out.zIndex = 2;
            break;
          case "near":
            out.zIndex = 1;
            break;
        }
        // Every selected node has the person's ring, dimmed or not, so what agents can see is on screen.
        if (selection.has(node) && node !== selected) {
          out.borderColor = palette.accent;
          out.borderSize = 0.32;
          out.zIndex = Math.max(Number(out.zIndex ?? 0), 2);
        }
        // The agent's halo: its own colour, never the selection's ring, and never on a dimmed node.
        if (agentMarks.has(node) && !selection.has(node) && node !== selected && out.color !== palette.dim) {
          out.borderColor = palette.agent;
          out.borderSize = 0.32;
          out.forceLabel = true;
          out.zIndex = Math.max(Number(out.zIndex ?? 0), 2);
        }
        // Merged (ws009 M2): every shown member is at its entity's centroid (applyMerge), so its edges are redrawn to it.
        // The head (or the selected member) is drawn on top with the merged title; the others are under it, unlabelled.
        const place = merge ? placesNow().get(node) : undefined;
        if (place) {
          out.size = place.size;
          const top = node === selected || (place.head && !(selected !== null && placesNow().get(selected)?.entity === place.entity));
          out.label = top ? place.title : null;
          if (!top) {
            out.forceLabel = false;
            out.highlighted = false;
            out.zIndex = 0;
          } else out.zIndex = Math.max(Number(out.zIndex ?? 0), 1) + 1;
        } else out.size = data.size;
        if (data.hub) {
          out.size = hubSize(out.size);
          out.forceLabel = true;
          out.zIndex = Math.max(Number(out.zIndex ?? 0), 2);
        }
        const clearance = place?.clearance ?? data.clearance;
        if (clearance !== undefined) {
          const [sizeRatio, sizeScale] = sizeProjection.value();
          const pixels = clearance * sizeRatio * 0.78;
          out.size = Math.min(out.size, pixels / sizeScale / extentOf(shapeAt(data.shape)).reach);
        }
        // A spine's anchors are always offered a label; the label pass cuts each to the room it has.
        if (spineAnchors.has(node) && out.label && out.color !== palette.dim) out.forceLabel = true;
        return out;
      },
      // Every edge's program, width and colour (plan 150): quiet relations thin in the structure colour, normal ones
      // 1.2 px; the selected node's and the focus's edges 1.8 px in the focus or relation colour (Instrument: ink).
      // The label is the relation; whether it is drawn is the edge-label pass's choice (drawEdgeLabels).
      edgeReducer: (edge, data) => {
        const out = edgeStyle(edge, data);
        if (!("type" in out)) return out;
        return { ...out, type: headProgram(out.type, out.size, incomingNow(graph.target(edge))) };
      },
    });
    function edgeStyle(edge: string, data: EdgeAttrs) {
      const [s, t] = graph.extremities(edge);
      const kind = data.kind;
      if (leftOut(edge)) return { ...data, hidden: true };
      const m = mode(kind);
      const curvature = curves.current.get(edge) ?? 0;
      const quiet = m === "quiet";
      const out = { ...data, type: edgeProgram(isSymmetric(edge, kind, symmetric), curvature), curvature, label: kind };
      if (focused) {
        if (inFocus(s, t, kind)) {
          const color = instrument ? palette.ink : kind === "part_of" ? palette.edgeStructure : relationColor(kind, palette.theme);
          return { ...out, color, size: EDGE_SIZE.emphasis, zIndex: 2 };
        }
        return { ...out, color: palette.edgeDim, size: quiet ? EDGE_SIZE.quiet : EDGE_SIZE.normal, zIndex: 0, label: null };
      }
      if (selected && (s === selected || t === selected)) return { ...out, color: selectedEdgeColor(kind, palette), size: EDGE_SIZE.emphasis, zIndex: 2 };
      return { ...out, color: quiet ? palette.edgeStructure : palette.edge, size: quiet ? EDGE_SIZE.quiet : EDGE_SIZE.normal };
    }
    const refreshSizes = sizeProjection.changed;
    sigma.getCamera().on("updated", refreshSizes);
    sigma.on("resize", refreshSizes);
    sigma.on("afterRender", refreshSizes);
    const sizeFrame = requestAnimationFrame(refreshSizes);
    return () => {
      cancelAnimationFrame(sizeFrame);
      sizeProjection.dispose();
      sigma.getCamera().off("updated", refreshSizes);
      sigma.off("resize", refreshSizes);
      sigma.off("afterRender", refreshSizes);
      sigma.off("beforeRender", newFrame);
      sigma.off("afterRender", drawEdgeLabels);
      sigma.off("enterNode", hoverLabels);
      sigma.off("leaveNode", hoverLabels);
      cancelAnimationFrame(hoverFrame);
    };
  }, [selected, selection, focused, groups, palette, graph, sigma, setSettings, offRelations, relationModes, symmetric, agentMarks, agentFocusNode, merge, placesNow, entities, proposedPairs, version, onUnlabelled, spineAxes]);

  // Shadow connections (ws009 M1c, ui-expert point 1): a dotted straight line under the edges, 0.8 px with 2 px dots and
  // 4 px gaps, no arrow, in the structure colour (Console: chrome-hi; Instrument: ink), fading in with the score. A
  // hovered node's top shadows join in Selected, each far end labelled with its score in mono.
  useEffect(() => {
    if (!sigma.getCanvases().shadows) {
      sigma.createCanvasContext("shadows", { beforeLayer: "edges" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const ctx = sigma.getCanvases().shadows.getContext("2d")!;
    const colour = palette.theme === "console" ? palette.chromeHi : palette.theme === "instrument" ? palette.ink : palette.structure;
    let hoveredNode: string | null = null;
    const draw = () => {
      const { width, height } = sigma.getDimensions();
      ctx.clearRect(0, 0, width, height);
      if (!shadows) return;
      const extra = hoveredNode && shadows.hover ? shadows.hover(hoveredNode) : [];
      const seen = new Set<string>();
      const buckets = new Map<number, [Pair, { x: number; y: number }, { x: number; y: number }][]>();
      for (const p of [...shadows.pairs, ...extra]) {
        const k = `${p.a}\u0000${p.b}`;
        if (seen.has(k) || !graph.hasNode(p.a) || !graph.hasNode(p.b)) continue;
        seen.add(k);
        const [s, t] = [onScreen(p.a), onScreen(p.b)];
        if ((s.x < 0 && t.x < 0) || (s.y < 0 && t.y < 0) || (s.x > width && t.x > width) || (s.y > height && t.y > height)) continue;
        const alpha = Math.round(shadowAlpha(p.score, p.floor) * 20) / 20;
        const list = buckets.get(alpha);
        if (list) list.push([p, s, t]);
        else buckets.set(alpha, [[p, s, t]]);
      }
      ctx.save();
      ctx.setLineDash([2, 4]);
      ctx.lineWidth = 0.8;
      ctx.strokeStyle = colour;
      for (const [alpha, lines] of buckets) {
        ctx.globalAlpha = alpha;
        ctx.beginPath();
        for (const [, s, t] of lines) {
          ctx.moveTo(s.x, s.y);
          ctx.lineTo(t.x, t.y);
        }
        ctx.stroke();
      }
      ctx.restore();
      if (!hoveredNode) return;
      ctx.save();
      ctx.font = `500 11px ${palette.mono}`;
      ctx.textBaseline = "middle";
      ctx.fillStyle = palette.ink;
      for (const lines of buckets.values()) {
        for (const [p] of lines) {
          if (p.a !== hoveredNode && p.b !== hoveredNode) continue;
          const far = onScreen(farEnd(p, hoveredNode));
          ctx.fillText(p.score.toFixed(2), far.x + far.r + 6, far.y - far.r - 6);
        }
      }
      ctx.restore();
    };
    const enter = ({ node }: { node: string }) => {
      hoveredNode = node;
      draw();
    };
    const leave = () => {
      hoveredNode = null;
      draw();
    };
    sigma.on("afterRender", draw);
    sigma.on("enterNode", enter);
    sigma.on("leaveNode", leave);
    sigma.refresh();
    return () => {
      sigma.off("afterRender", draw);
      sigma.off("enterNode", enter);
      sigma.off("leaveNode", leave);
    };
  }, [sigma, graph, shadows, palette, onScreen]);

  // Entities (ws009 M2, design point 1): a soft curve per member pair under the edges, in the structure colour, no
  // arrow; proposed pairs dashed and quiet, never merged. Merged, the confirmed threads are hidden and each entity's
  // top node gets a ring in its members' graph colours, above the nodes. Both redraw after each sigma render.
  useEffect(() => {
    if (!sigma.getCanvases().threads) {
      sigma.createCanvasContext("threads", { beforeLayer: "edges" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.createCanvasContext("entityRings", { beforeLayer: "labels" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const ctx = sigma.getCanvases().threads.getContext("2d")!;
    const rings = sigma.getCanvases().entityRings.getContext("2d")!;
    const shown = (id: string) => graph.hasNode(id) && !graph.getNodeAttribute(id, "hidden");
    const lit = (id: string) => !focused || focused.has(id);
    const curve = (a: { x: number; y: number }, b: { x: number; y: number }) => {
      const [mx, my] = [(a.x + b.x) / 2, (a.y + b.y) / 2];
      const [dx, dy] = [b.x - a.x, b.y - a.y];
      ctx.moveTo(a.x, a.y);
      ctx.quadraticCurveTo(mx - dy * 0.15, my + dx * 0.15, b.x, b.y);
    };
    const el = sigma.getContainer();
    const draw = () => {
      const { width, height } = sigma.getDimensions();
      ctx.clearRect(0, 0, width, height);
      rings.clearRect(0, 0, width, height);
      // How many of "Show across graphs"'s members are on screen (data-across-in-view), for the browser check.
      if (across) {
        el.dataset.acrossInView = String([...across].filter((id) => shown(id) && ((p) => p.x >= 0 && p.y >= 0 && p.x <= width && p.y <= height)(onScreen(id))).length);
      } else delete el.dataset.acrossInView;
      if (entities.list.length === 0 && entities.proposed.length === 0) return;
      const off = (a: { x: number; y: number }, b: { x: number; y: number }) =>
        (a.x < 0 && b.x < 0) || (a.y < 0 && b.y < 0) || (a.x > width && b.x > width) || (a.y > height && b.y > height);
      ctx.lineCap = "round";
      const stroke = (pairs: [string, string][], color: string, size: number) => {
        ctx.beginPath();
        for (const [s, t] of pairs) {
          const [a, b] = [onScreen(s), onScreen(t)];
          if (!off(a, b)) curve(a, b);
        }
        ctx.strokeStyle = color;
        ctx.lineWidth = size;
        ctx.stroke();
      };
      if (!merge) {
        const plain: [string, string][] = [];
        const dim: [string, string][] = [];
        const picked: [string, string][] = [];
        for (const e of entities.list) {
          for (const [s, t] of threadPairs(e.members.filter(shown))) {
            if (across?.has(s) && across.has(t)) picked.push([s, t]);
            else if (lit(s) && lit(t)) plain.push([s, t]);
            else dim.push([s, t]);
          }
        }
        stroke(dim, palette.edgeDim, THREAD.normal);
        stroke(plain, palette.structure, THREAD.normal);
        stroke(picked, palette.accent, THREAD.emphasis);
      }
      ctx.setLineDash([4, 4]);
      stroke(entities.proposed.filter(([s, t]) => shown(s) && shown(t)), palette.edgeStructure, 1);
      ctx.setLineDash([]);
      if (!merge || !colorOf) return;
      for (const [id, p] of placesNow()) {
        if (!p.head) continue;
        const { x, y, r } = onScreen(id);
        if (x < -r || y < -r || x > width + r || y > height + r) continue;
        const n = p.graphs.length;
        const gap = n > 1 ? 0.12 : 0;
        // A canvas-coloured band under the segments keeps them apart from the node and from what is behind.
        rings.beginPath();
        rings.arc(x, y, r + 4, 0, Math.PI * 2);
        rings.strokeStyle = palette.canvas;
        rings.lineWidth = 7;
        rings.stroke();
        p.graphs.forEach((g, i) => {
          rings.beginPath();
          rings.arc(x, y, r + 4, (i / n) * Math.PI * 2 - Math.PI / 2 + gap / 2, ((i + 1) / n) * Math.PI * 2 - Math.PI / 2 - gap / 2);
          rings.strokeStyle = colorOf(g);
          rings.lineWidth = 4;
          rings.stroke();
        });
      }
    };
    sigma.on("afterRender", draw);
    sigma.refresh();
    return () => {
      sigma.off("afterRender", draw);
    };
  }, [sigma, graph, entities, merge, across, focused, palette, colorOf, onScreen, placesNow]);

  // FUI skins (K1, design note sections 4 to 6): glow under the edges and callouts on leaders above the group pills,
  // each on its own 2D layer, redrawn after every sigma render. Glow is never WebGL alpha (premultiplied blending
  // draws light colours near white on dark). A short rAF loop runs only while a live glow or a callout is fading in.
  const calloutStarts = useRef(new Map<string, number>());
  const readoutRef = useRef<HTMLDivElement>(null);
  const [canvasReadouts, setCanvasReadouts] = useState<CanvasReadouts>({ zoom: 100, glow: [0, 0], callouts: [0, 0] });
  useEffect(() => {
    if (!fui) return;
    if (!sigma.getCanvases().glow) {
      sigma.createCanvasContext("glow", { beforeLayer: "edges" } as { style?: Partial<CSSStyleDeclaration> });
      // Above sigma's labels, so a callout is never written over; under the hover layer.
      sigma.createCanvasContext("callouts", { beforeLayer: "hovers" } as { style?: Partial<CSSStyleDeclaration> });
      sigma.resize(true);
    }
    const glowCtx = sigma.getCanvases().glow.getContext("2d")!;
    const calloutCtx = sigma.getCanvases().callouts.getContext("2d")!;
    const instrument = palette.theme === "instrument";
    const shown = (id: string) => graph.hasNode(id) && !graph.getNodeAttribute(id, "hidden");
    const undimmed = (id: string) => shown(id) && (!focused || focused.has(id));
    const ordered = selected && selection.has(selected) ? [selected, ...[...selection].filter((id) => id !== selected)] : [...selection];
    const agentIds = agentFocusNode && !agentMarks.has(agentFocusNode) ? [...agentMarks, agentFocusNode] : [...agentMarks];
    const colour = { select: palette.glowSelect, agent: palette.glowAgent, live: palette.glowLive };
    const viewport = onScreen;
    let frame = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const report = (next: CanvasReadouts) =>
      setCanvasReadouts((was) =>
        was.zoom === next.zoom && was.glow[0] === next.glow[0] && was.glow[1] === next.glow[1] && was.callouts[0] === next.callouts[0] && was.callouts[1] === next.callouts[1]
          ? was
          : next,
      );

    const draw = () => {
      const now = performance.now();
      const { width, height } = sigma.getDimensions();
      glowCtx.clearRect(0, 0, width, height);
      calloutCtx.clearRect(0, 0, width, height);
      let animating = false;

      // Glow: selection in the accent, the agent's nodes in its blue, a live change in the accent, fading.
      const strength = live ? liveGlow(now - live.at, reduced) : 0;
      if (strength > 0 && !reduced) animating = true;
      // With reduced motion the live glow is on, unfaded, then gone: one redraw when its time is up.
      if (strength > 0 && reduced && timer === undefined) timer = setTimeout(draw, LIVE_TOTAL - (now - live!.at) + 5);
      const { glow, wanted } = glowSet(ordered.filter(shown), agentIds.filter(undimmed), strength > 0 ? live!.ids.filter(undimmed) : [], shown);
      for (const [id, kind] of glow) {
        const { x, y, r } = viewport(id);
        if (x < -r * 3 || y < -r * 3 || x > width + r * 3 || y > height + r * 3) continue;
        const k = kind === "live" ? strength : 1;
        const shape = shapeAt(graph.getNodeAttribute(id, "shape"));
        // The glow leaves from the node's outline (plan 198): painted outside its shape only, so the space between a
        // shape and its circle takes the gradient's inner colour.
        glowCtx.save();
        glowCtx.beginPath();
        glowCtx.rect(0, 0, width, height);
        shapePath(glowCtx, shape, x, y, r);
        glowCtx.clip("evenodd");
        if (instrument) {
          // Light is not dark inverted (rule 8): a soft tint halo and a 2 px ink ring, no glow.
          const halo = glowCtx.createRadialGradient(x, y, r, x, y, r * 2.5 + 4);
          halo.addColorStop(0, withAlpha(colour[kind], 0.25 * k));
          halo.addColorStop(1, withAlpha(colour[kind], 0));
          glowCtx.fillStyle = halo;
          glowCtx.beginPath();
          glowCtx.arc(x, y, r * 2.5 + 4, 0, Math.PI * 2);
          glowCtx.fill();
          glowCtx.globalAlpha = k;
          glowCtx.strokeStyle = palette.ink;
          glowCtx.lineWidth = 2;
          glowCtx.beginPath();
          shapePath(glowCtx, shape, x, y, r + 2);
          glowCtx.stroke();
        } else {
          const { inner, outer: radius } = glowRadii(r);
          const g = glowCtx.createRadialGradient(x, y, inner, x, y, radius);
          g.addColorStop(0, withAlpha(colour[kind], 0.45 * k));
          g.addColorStop(1, withAlpha(colour[kind], 0));
          glowCtx.fillStyle = g;
          glowCtx.beginPath();
          glowCtx.arc(x, y, radius, 0, Math.PI * 2);
          glowCtx.fill();
        }
        glowCtx.restore();
      }

      // Callouts: selection first, then the agent's nodes by event order, at most LEADER_CAP.
      const right = width - (panelOpen ? Math.min(PANEL_W, window.innerWidth * 0.9) : 0);
      const readoutBottom = readoutRef.current ? readoutRef.current.offsetTop + readoutRef.current.offsetHeight : 0;
      const frameBox: Frame = {
        width,
        height,
        left: { edge: 0, top: 36, bottom: 76 },
        right: { edge: right, top: Math.max(16, readoutBottom + 12), bottom: panelOpen ? 16 : 132 },
        gap: 6,
      };
      const colW = columnWidth(frameBox);
      const inner = colW - 20;
      const titleFont = `500 12px ${palette.font}`;
      const idFont = `400 11px ${palette.mono}`;
      const noteFont = `italic 400 11px ${palette.font}`;
      // Each line cut at a grapheme to the box's inner width, so nothing is drawn past the box (review B1); measured
      // before the boxes are placed, so no box covers its node or the selected one (ui-expert K1-N1).
      const fit = (text: string, font: string) => {
        calloutCtx.font = font;
        const out = fitCallout(text, inner, (t) => calloutCtx.measureText(t).width);
        return { text: out, w: calloutCtx.measureText(out).width };
      };
      const lines = new Map<string, { title: { text: string; w: number }; id: { text: string; w: number }; noteLine: { text: string; w: number } | null; w: number }>();
      const candidates: LeaderCandidate[] = [];
      for (const [ids, kind] of [[ordered, "select"], [agentIds, "agent"]] as const) {
        for (const id of ids) {
          if (!(kind === "select" ? shown(id) : undimmed(id))) continue;
          const { x, y, r } = viewport(id);
          candidates.push({ id, kind, x, y, r, h: agentNotes.has(id) ? 60 : 46 });
        }
      }
      // Only the callouts that can be placed are measured.
      const { chosen, beyond } = leaderChoice(candidates, frameBox);
      for (const c of chosen) {
        const note = agentNotes.get(c.id);
        const title = fit(graph.getNodeAttribute(c.id, "label") ?? "", titleFont);
        const idLine = fit(c.id, idFont);
        const noteLine = note ? fit(note, noteFont) : null;
        lines.set(c.id, { title, id: idLine, noteLine, w: Math.min(colW, Math.max(title.w, idLine.w, noteLine?.w ?? 0) + 20) });
        c.w = lines.get(c.id)!.w;
      }
      const keepClear = selected && shown(selected) ? viewport(selected) : null;
      const result = placeLeaders(chosen, { ...frameBox, keepClear });
      const { placed } = result;
      const more = result.more + beyond;
      // Where text may be drawn on this layer, for the browser check (numbers only).
      const boxes: [number, number, number, number][] = [];
      const starts = calloutStarts.current;
      const keep = new Set(placed.map((p) => p.id));
      for (const id of [...starts.keys()]) if (!keep.has(id)) starts.delete(id);
      calloutCtx.textBaseline = "alphabetic";
      // Each callout's box and lines first; then every leader, so no leader is drawn over another callout's box.
      const laid = placed.map((p) => {
        if (!starts.has(p.id)) starts.set(p.id, now);
        const t = calloutIn(now - starts.get(p.id)!, reduced);
        if (t.text < 1) animating = true;
        const { title, id, noteLine, w } = lines.get(p.id)!;
        const bx = p.side === "left" ? frameBox.left.edge + COLUMN_INSET : frameBox.right.edge - COLUMN_INSET - w;
        boxes.push([Math.floor(bx), Math.floor(p.top), Math.ceil(w), p.h]);
        return { p, t, title, id, noteLine, w, bx, by: p.top };
      });
      for (const { p, t, w, bx, by } of laid) {
        const { x, y, r } = viewport(p.id);
        // The leader runs to the box point nearest its node, under the boxes (ui-expert K1-N1).
        const { x: ex, y: ey } = leaderEnd({ x, y }, { x: bx, y: by, w, h: p.h });
        const len = Math.hypot(ex - x, ey - y) || 1;
        const sx = x + ((ex - x) / len) * (r + 3);
        const sy = y + ((ey - y) / len) * (r + 3);
        calloutCtx.beginPath();
        calloutCtx.moveTo(sx, sy);
        calloutCtx.lineTo(sx + (ex - sx) * t.leader, sy + (ey - sy) * t.leader);
        calloutCtx.lineWidth = 1;
        calloutCtx.strokeStyle = p.kind === "select" ? palette.glowSelect : palette.glowAgent;
        calloutCtx.stroke();
      }
      for (const { p, t, title, id, noteLine, w, bx, by } of laid) {
        if (t.text <= 0) continue;
        calloutCtx.globalAlpha = t.text;
        calloutCtx.fillStyle = palette.halo;
        calloutCtx.fillRect(bx, by, w, p.h);
        calloutCtx.strokeStyle = palette.chrome;
        calloutCtx.strokeRect(bx + 0.5, by + 0.5, w - 1, p.h - 1);
        calloutCtx.strokeStyle = palette.chromeHi;
        calloutCtx.beginPath();
        calloutCtx.moveTo(bx + 0.5, by + 8.5);
        calloutCtx.lineTo(bx + 0.5, by + 0.5);
        calloutCtx.lineTo(bx + 8.5, by + 0.5);
        calloutCtx.moveTo(bx + w - 8.5, by + p.h - 0.5);
        calloutCtx.lineTo(bx + w - 0.5, by + p.h - 0.5);
        calloutCtx.lineTo(bx + w - 0.5, by + p.h - 8.5);
        calloutCtx.stroke();
        calloutCtx.fillStyle = p.kind === "select" ? palette.glowSelect : palette.glowAgent;
        calloutCtx.fillRect(p.side === "left" ? bx + w - 2 : bx, by + 6, 2, p.h - 12);
        calloutCtx.save();
        calloutCtx.beginPath();
        calloutCtx.rect(bx, by, w, p.h);
        calloutCtx.clip();
        calloutCtx.textAlign = "left";
        calloutCtx.fillStyle = palette.ink;
        calloutCtx.font = titleFont;
        calloutCtx.fillText(title.text, bx + 10, by + 19);
        calloutCtx.fillStyle = palette.structure;
        calloutCtx.font = idFont;
        calloutCtx.fillText(id.text, bx + 10, by + 35);
        if (noteLine) {
          calloutCtx.font = noteFont;
          calloutCtx.fillText(noteLine.text, bx + 10, by + 50);
        }
        calloutCtx.restore();
        calloutCtx.globalAlpha = 1;
      }
      if (more > 0) {
        calloutCtx.textAlign = "left";
        calloutCtx.fillStyle = palette.chromeText;
        calloutCtx.font = `500 11px ${palette.mono}`;
        calloutCtx.fillText(moreCallouts(more), frameBox.left.edge + COLUMN_INSET, 24);
        boxes.push([frameBox.left.edge + COLUMN_INSET, 10, Math.ceil(calloutCtx.measureText(moreCallouts(more)).width) + 2, 18]);
      }
      const layer = sigma.getCanvases().callouts;
      const json = JSON.stringify(boxes);
      if (layer.dataset.boxes !== json) layer.dataset.boxes = json;
      report({ zoom: Math.round(100 / sigma.getCamera().ratio), glow: [glow.length, wanted], callouts: [placed.length, more] });
      if (animating && !frame) {
        frame = requestAnimationFrame(() => {
          frame = 0;
          draw();
        });
      }
    };
    sigma.on("afterRender", draw);
    draw();
    return () => {
      sigma.off("afterRender", draw);
      if (frame) cancelAnimationFrame(frame);
      clearTimeout(timer);
      const { width, height } = sigma.getDimensions();
      glowCtx.clearRect(0, 0, width, height);
      calloutCtx.clearRect(0, 0, width, height);
    };
  }, [fui, sigma, graph, palette, selected, selection, focused, agentMarks, agentFocusNode, agentNotes, live, reduced, panelOpen, version, onScreen]);

  // A jump runs once, as soon as its node is shown: a result revealed from the sidebar is hidden until the app has
  // applied the new visibility (a parent effect, after this one), so `version` brings it back here.
  const jumped = useRef(0);
  useEffect(() => {
    if (!jump || jump.n === jumped.current || !graph.hasNode(jump.id) || graph.getNodeAttribute(jump.id, "hidden")) return;
    jumped.current = jump.n;
    // From graph to framed coordinates; the display cache can hold raw positions while a refresh is pending.
    const { x, y } = graph.getNodeAttributes(jump.id);
    const target = sigma.viewportToFramedGraph(sigma.graphToViewport({ x, y }));
    const camera = sigma.getCamera();
    void camera
      .animate({ x: target.x, y: target.y, ratio: Math.min(camera.ratio, 0.4) }, { duration: ease(500) })
      .then(() => onJumped?.(jump.id));
  }, [jump, graph, sigma, version, onJumped, ease]);

  return (
    <>
      {fui && readouts ? (
        <div ref={readoutRef} className={cn("pointer-events-none absolute top-4 z-10", panelOpen ? "right-[436px]" : "right-4")}>
          <Readouts values={readouts} canvas={canvasReadouts} />
        </div>
      ) : null}
      <div className="fui-frame absolute right-4 bottom-4 flex flex-col gap-1 rounded-lg border bg-card/90 p-1 shadow-sm backdrop-blur">
        <span className="fui-brackets" aria-hidden />
        <Tip label="Zoom in">
          <Button variant="ghost" size="icon-sm" aria-label="Zoom in" onClick={() => sigma.getCamera().animatedZoom({ duration: ease(250) })}>
            <Plus />
          </Button>
        </Tip>
        <Tip label="Zoom out">
          <Button variant="ghost" size="icon-sm" aria-label="Zoom out" onClick={() => sigma.getCamera().animatedUnzoom({ duration: ease(250) })}>
            <Minus />
          </Button>
        </Tip>
        <Tip label="Fit the graph">
          <Button variant="ghost" size="icon-sm" aria-label="Fit" data-testid="fit" onClick={() => fitAll.current(ease(400))}>
            <Maximize />
          </Button>
        </Tip>
      </div>
    </>
  );
}

export default function GraphCanvas(props: Props) {
  // Fixed at mount; Behaviour updates the theme-dependent settings.
  const [settings] = useState<Partial<Settings<NodeAttrs, EdgeAttrs>>>(() => ({
      // One node program draws every shape (plan 198, lib/graph/shape-program): fill, status ring and selection or
      // agent ring.
      defaultNodeType: "shape",
      nodeProgramClasses: { shape: ShapeProgram<NodeAttrs, EdgeAttrs> },
      defaultEdgeType: "arrow",
      edgeProgramClasses: EdgePrograms,
      // Edge labels are Behaviour's own pass (drawEdgeLabels, plan 198), drawn on sigma's edge-label layer.
      renderEdgeLabels: false,
      zIndex: true,
      labelDensity: labelDensity(props.graph.order),
      labelGridCellSize: 200,
      labelRenderedSizeThreshold: labelThreshold(props.graph.order),
      labelSize: 12,
      labelWeight: "500",
      labelFont: props.palette.font,
      labelColor: { color: props.palette.ink },
      minCameraRatio: 0.02,
      maxCameraRatio: 8,
      stagePadding: 40,
      hideEdgesOnMove: false,
  }));
  return (
    <SigmaContainer<NodeAttrs, EdgeAttrs> graph={props.graph} settings={settings} className="sigma-host">
      <Behaviour {...props} />
    </SigmaContainer>
  );
}

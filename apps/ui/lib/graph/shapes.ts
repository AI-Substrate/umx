// Node shapes by type (plan 198; the UI expert's design note, section 1, review B1). One geometry per
// shape, in units of the node's radius: each keeps the disc's visual weight, spanning at least the full diameter
// across, at least 0.7 of it tall, with an area within 15% of the disc's, so sizes still compare. The WebGL program
// (lib/graph/shape-program.ts) draws it as a signed distance, the 2D layers (hover ring, Console glow, selection halo)
// trace it with shapePath, and a type chip's swatch is its svgPath: three readers, one table. Pure.

/** The shapes, by the number the node program reads from a node's `shape` attribute. */
export const SHAPES = ["roundRect", "circle", "ellipse", "hexagon", "diamond", "tag", "cutRect", "pill"] as const;
export type Shape = (typeof SHAPES)[number];
export const DEFAULT_SHAPE: Shape = "roundRect";

/** Types to shapes (D42: the UI's table, never the vocabulary's). A type not listed is a rounded rectangle. */
const BY_TYPE: Readonly<Record<string, Shape>> = {
  person: "circle",
  // The hidden provenance types (D55) draw as people do.
  session: "circle",
  agent: "circle",
  harness: "circle",
  model: "circle",
  platform: "circle",
  team: "ellipse",
  organisation: "ellipse",
  role: "ellipse",
  option: "hexagon",
  question: "diamond",
  decision: "tag",
  problem: "cutRect",
  risk: "cutRect",
  tension: "cutRect",
  meeting: "pill",
  event: "pill",
  segment: "pill",
};

export function shapeOf(type: string): Shape {
  return Object.hasOwn(BY_TYPE, type) ? BY_TYPE[type] : DEFAULT_SHAPE;
}

/** The number the node program reads (an index in SHAPES). */
export function shapeIndex(type: string): number {
  return SHAPES.indexOf(shapeOf(type));
}

/** The shape a node's `shape` attribute names; anything else (missing, out of range) is the default. */
export function shapeAt(index: unknown): Shape {
  return (typeof index === "number" && SHAPES[index]) || DEFAULT_SHAPE;
}

type Point = readonly [number, number];

/**
 * A shape's geometry, in node radii. `poly`: a convex polygon (two points make a segment) grown by `round`, so its
 * outline is the polygon's edges pushed out by `round` with arcs at the corners. Its points run by increasing angle
 * (counter-clockwise with y up): the 2D path turns each corner that way and the program's inside test assumes it.
 * `circle` and `ellipse` are analytic.
 */
export type Geometry = { kind: "circle" } | { kind: "ellipse"; rx: number; ry: number } | { kind: "poly"; points: readonly Point[]; round: number };

const hexagon = (r: number): Point[] =>
  Array.from({ length: 6 }, (_, i) => [Math.round(r * Math.cos((i * Math.PI) / 3) * 1e4) / 1e4, Math.round(r * Math.sin((i * Math.PI) / 3) * 1e4) / 1e4] as Point);

export const GEOMETRY: Readonly<Record<Shape, Geometry>> = {
  roundRect: { kind: "poly", points: [[-0.7, -0.48], [0.7, -0.48], [0.7, 0.48], [-0.7, 0.48]], round: 0.3 },
  circle: { kind: "circle" },
  ellipse: { kind: "ellipse", rx: 1.1, ry: 0.85 },
  hexagon: { kind: "poly", points: hexagon(0.95), round: 0.15 },
  diamond: { kind: "poly", points: [[0.8, 0], [0, 0.8], [-0.8, 0], [0, -0.8]], round: 0.3 },
  tag: { kind: "poly", points: [[-0.9, -0.65], [0.4, -0.65], [0.9, 0], [0.4, 0.65], [-0.9, 0.65]], round: 0.15 },
  cutRect: { kind: "poly", points: [[-0.7, -0.75], [0.7, -0.75], [1, -0.45], [1, 0.45], [0.7, 0.75], [-0.7, 0.75], [-1, 0.45], [-1, -0.45]], round: 0 },
  pill: { kind: "poly", points: [[-0.35, 0], [0.35, 0]], round: 0.75 },
};

/** A shape's half width and half height, its farthest point from the centre, and its area, in node radii. */
export type Extent = { halfWidth: number; halfHeight: number; reach: number; area: number };

export function extentOf(shape: Shape): Extent {
  const g = GEOMETRY[shape];
  if (g.kind === "circle") return { halfWidth: 1, halfHeight: 1, reach: 1, area: Math.PI };
  if (g.kind === "ellipse") return { halfWidth: g.rx, halfHeight: g.ry, reach: Math.max(g.rx, g.ry), area: Math.PI * g.rx * g.ry };
  const { points, round } = g;
  let [area, perimeter] = [0, 0];
  points.forEach((a, i) => {
    const b = points[(i + 1) % points.length];
    area += (a[0] * b[1] - b[0] * a[1]) / 2;
    perimeter += Math.hypot(b[0] - a[0], b[1] - a[1]);
  });
  // Growing a convex polygon by `round` adds its perimeter times `round` and a disc of radius `round`.
  return {
    halfWidth: Math.max(...points.map((p) => Math.abs(p[0]))) + round,
    halfHeight: Math.max(...points.map((p) => Math.abs(p[1]))) + round,
    reach: Math.max(...points.map((p) => Math.hypot(p[0], p[1]))) + round,
    area: Math.abs(area) + perimeter * round + Math.PI * round * round,
  };
}

/** The farthest any shape reaches, in node radii: the node program's triangle must cover it. */
export const SHAPE_REACH = Math.max(...SHAPES.map((s) => extentOf(s).reach));

/** The outward normal of each edge i (points[i] to points[i + 1]) of a convex polygon, as an angle. A segment's two
 * edges run opposite ways, so their normals already face opposite sides. */
function normals(points: readonly Point[]): number[] {
  const [cx, cy] = points.reduce(([x, y], p) => [x + p[0] / points.length, y + p[1] / points.length], [0, 0]);
  return points.map((a, i) => {
    const b = points[(i + 1) % points.length];
    let [nx, ny] = [b[1] - a[1], a[0] - b[0]];
    if (nx * ((a[0] + b[0]) / 2 - cx) + ny * ((a[1] + b[1]) / 2 - cy) < 0) [nx, ny] = [-nx, -ny];
    return Math.atan2(ny, nx);
  });
}

type PathSink = { moveTo(x: number, y: number): void; lineTo(x: number, y: number): void; arc(x: number, y: number, r: number, a0: number, a1: number): void; closePath(): void; ellipse(x: number, y: number, rx: number, ry: number, rotation: number, a0: number, a1: number): void };

/** The path of a polygon grown by `round`: each corner an arc from the incoming edge's normal to the outgoing one's.
 * The points run by increasing angle (tests hold every table row to it), so each arc turns the short way. It starts a
 * new subpath (moveTo) at the first arc's start: an arc alone would join the subpath before it with a line, cutting
 * a wedge out of a clip such as the glow's (reviewer R370-1). */
function tracePoly(sink: PathSink, points: readonly Point[], round: number, x: number, y: number, r: number) {
  const n = normals(points);
  points.forEach((p, i) => {
    const [vx, vy] = [x + p[0] * r, y + p[1] * r];
    if (round > 0) {
      const from = n[(i + points.length - 1) % points.length];
      let to = n[i];
      while (to < from) to += Math.PI * 2;
      if (i === 0) sink.moveTo(vx + round * r * Math.cos(from), vy + round * r * Math.sin(from));
      sink.arc(vx, vy, round * r, from, to);
    } else if (i === 0) sink.moveTo(vx, vy);
    else sink.lineTo(vx, vy);
  });
  sink.closePath();
}

/**
 * Traces `shape` centred at (x, y) with radius `r` as a new subpath (the caller begins the path and fills, strokes or
 * clips). A canvas's y runs down, the shapes are symmetric about their horizontal axis, so they read the same.
 */
export function shapePath(ctx: PathSink, shape: Shape, x: number, y: number, r: number) {
  const g = GEOMETRY[shape];
  if (g.kind === "circle") {
    ctx.moveTo(x + r, y);
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.closePath();
  } else if (g.kind === "ellipse") {
    ctx.moveTo(x + g.rx * r, y);
    ctx.ellipse(x, y, g.rx * r, g.ry * r, 0, 0, Math.PI * 2);
    ctx.closePath();
  } else tracePoly(ctx, g.points, g.round, x, y, r);
}

const fmt = (v: number) => String(Math.round(v * 1000) / 1000);

/** The SVG path of `shape` in a box `size` wide, centred (a type chip's swatch). */
export function svgPath(shape: Shape, size: number): string {
  const parts: string[] = [];
  let at: [number, number] | null = null;
  const c = size / 2;
  const sink: PathSink = {
    moveTo: (x, y) => {
      parts.push(`M${fmt(x)} ${fmt(y)}`);
      at = [x, y];
    },
    lineTo: (x, y) => {
      parts.push(`L${fmt(x)} ${fmt(y)}`);
      at = [x, y];
    },
    arc: (x, y, r, a0, a1) => {
      const [sx, sy, ex, ey] = [x + r * Math.cos(a0), y + r * Math.sin(a0), x + r * Math.cos(a1), y + r * Math.sin(a1)];
      parts.push(`${at ? "L" : "M"}${fmt(sx)} ${fmt(sy)}`);
      // A full turn is two half arcs (SVG cannot draw one arc back to its start).
      if (a1 - a0 >= Math.PI * 2 - 1e-9) parts.push(`A${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(x - r * Math.cos(a0))} ${fmt(y - r * Math.sin(a0))}`, `A${fmt(r)} ${fmt(r)} 0 0 1 ${fmt(ex)} ${fmt(ey)}`);
      else parts.push(`A${fmt(r)} ${fmt(r)} 0 ${a1 - a0 > Math.PI ? 1 : 0} 1 ${fmt(ex)} ${fmt(ey)}`);
      at = [ex, ey];
    },
    ellipse: (x, y, rx, ry) => {
      parts.push(`M${fmt(x + rx)} ${fmt(y)}`, `A${fmt(rx)} ${fmt(ry)} 0 0 1 ${fmt(x - rx)} ${fmt(y)}`, `A${fmt(rx)} ${fmt(ry)} 0 0 1 ${fmt(x + rx)} ${fmt(y)}`);
      at = [x + rx, y];
    },
    closePath: () => parts.push("Z"),
  };
  // Shapes reach past the node's radius (wide ones across, pointed ones at a tip): fit the widest side to the box.
  const { halfWidth, halfHeight } = extentOf(shape);
  shapePath(sink, shape, c, c, c / Math.max(halfWidth, halfHeight));
  return parts.join("");
}

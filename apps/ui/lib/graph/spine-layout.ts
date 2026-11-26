// The spine view (Jordan's brief part 2): a graph laid out along one of its declared spines. umx places each node
// (`node list --columns spine:<name>`, its one placement rule); this only turns those bands into positions. Earliest
// band on the left; each anchor on the centre line in its band; the nodes placed in a band beside it, above and below
// the line as the force layout had them; nodes the spine does not place on a shelf under the lane (the canvas's y grows
// upward). The lane keeps the
// graph's own region, so several graphs each keep theirs.

/** Where umx placed one node: its band (null when not placed), how, and whether it is one of the spine's anchors. */
export type Place = { band: number | null; by: string; anchor: boolean };

/** One graph's spine line, in graph coordinates, for the canvas to draw: from the first band to the last. `anchors` are
 * the anchors' canvas keys (their labels come first), `pitch` the gap between two rows of a band (a label's height on
 * screen at the zoom where every row can carry one). */
export type SpineAxis = { graph: string; y: number; x0: number; x1: number; bands: number[]; anchors: string[]; pitch: number };

type At = { x: number; y: number };
const NO_HUBS: ReadonlyMap<string, string> = new Map();

/** The lane's width over its height when the canvas does not say: a wide browser canvas. */
export const ASPECT = 1.8;
/** At spread 1, the gap between two rows of a band is the force layout's height over this. */
const ROWS = 24;
/** Sub-columns of a band sit this many row gaps apart at spread 1, room for a short label. */
const FAN = 1.8;
/** Bands sit this many sub-columns apart past their own width. */
const GAP = 0.6;

/** How many sub-columns a band of `n` nodes fans into when each holds at most `rows` of them (half above the line,
 * half below). */
export function fanOf(n: number, rows: number): number {
  return n <= 1 ? n : Math.ceil(n / Math.max(2, rows));
}

/** How far a band's `k` sub-columns sit right of its anchor, in sub-columns: an odd number is moved half a column, so
 * none sits on the anchor's own column and its edges to the anchor do not run along each other. */
const shiftOf = (k: number) => (k % 2) / 2;

/** A band's width in sub-columns: its fan (or its anchors side by side), the odd fan's shift and the gap to the next. */
const spanOf = (k: number, anchors: number) => Math.max(k + shiftOf(k), anchors, 1) + GAP;

/** The Spread slider (spine view): it widens the gap between bands and between the nodes of a band. */
export const SPREAD = { min: 0.5, max: 4, step: 0.1, default: 1 } as const;

/** A spread in range, else the default. */
export function spreadOf(v: number | undefined): number {
  return v !== undefined && Number.isFinite(v) && v >= SPREAD.min && v <= SPREAD.max ? v : SPREAD.default;
}

/**
 * The rows per side of the line that let Fit show the lane biggest, for bands of `sizes` nodes with `wide` anchors
 * each and `shelf` nodes not placed, columns `col` apart and rows `row` apart, on a canvas `aspect` wide over high:
 * the lane that needs the least zoom out to fit, so its rows are furthest apart on screen. Taller bands mean fewer
 * sub-columns, so a big band grows the lane up and down as well as across, as far as the canvas's shape allows.
 */
export function rowsFor(sizes: readonly number[], wide: readonly number[], shelf: number, col: number, row: number, aspect: number): number {
  const most = Math.max(1, ...sizes);
  let [best, least] = [1, Infinity];
  for (let r = 1; r <= Math.ceil(most / 2); r++) {
    const cols = sizes.reduce((sum, n, i) => sum + spanOf(fanOf(n, 2 * r), wide[i] ?? 0), 0);
    const shelfRows = shelf > 0 ? 2 + Math.ceil(shelf / Math.max(1, Math.floor(cols))) : 0;
    const zoom = Math.max((cols * col) / aspect, (2 * r + 1 + shelfRows) * row);
    if (zoom < least) [best, least] = [r, zoom];
  }
  return best;
}

/**
 * Positions for one graph's nodes along a spine. `nodes` are the graph's nodes with their force-layout positions
 * (`key` the canvas key, `id` umx's id); `places` umx's placements by id; `spread` the Spread slider; `aspect` the
 * canvas's width over its height. Returns each node's position and the axis. Each band is a grid around its anchor:
 * sub-columns kept in the force layout's order across, each node above or below the line as the force layout had it
 * within its sub-column, one row gap apart, never on the line. The number of rows is picked so the whole lane has the
 * canvas's shape; each band takes the width its sub-columns need, so neighbouring bands never meet. A wider spread
 * moves rows and bands further apart.
 */
export function spineLayout(
  graph: string,
  nodes: readonly { key: string; id: string; x: number; y: number }[],
  places: ReadonlyMap<string, Place>,
  spread: number = SPREAD.default,
  aspect: number = ASPECT,
  hubs: ReadonlyMap<string, string> = NO_HUBS,
  spacing = 1.6,
): { at: Map<string, At>; axis: SpineAxis | null; clearance: number } {
  const at = new Map<string, At>();
  if (nodes.length === 0) return { at, axis: null, clearance: 1 };
  const s = spreadOf(spread);
  let [x0, x1, y0, y1] = [Infinity, -Infinity, Infinity, -Infinity];
  for (const n of nodes) {
    x0 = Math.min(x0, n.x);
    x1 = Math.max(x1, n.x);
    y0 = Math.min(y0, n.y);
    y1 = Math.max(y1, n.y);
  }
  const h = Math.max(y1 - y0, 1);
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  // Spread pushes rows apart by s and columns by sqrt(s): labels run across, so rows need the room more.
  const row = (h / ROWS) * s * Math.sqrt(spacing / 1.6);
  const col = (h / ROWS) * FAN * Math.sqrt(s) * Math.sqrt(spacing / 1.6);
  if (hubs.size > 0) return spineFans(graph, nodes, places, hubs, row, col, cx, cy);

  const bands = [...new Set(nodes.map((n) => places.get(n.id)?.band).filter((b): b is number => typeof b === "number"))].sort((a, b) => a - b);
  const members = new Map<number, { key: string; x: number; y: number }[]>();
  const anchors = new Map<number, string[]>();
  const shelf: { key: string; x: number }[] = [];
  for (const n of nodes) {
    const p = places.get(n.id);
    if (!p || p.band === null) shelf.push({ key: n.key, x: n.x });
    else if (p.anchor) {
      const list = anchors.get(p.band) ?? [];
      list.push(n.key);
      anchors.set(p.band, list);
    } else {
      const list = members.get(p.band) ?? [];
      list.push(n);
      members.set(p.band, list);
    }
  }
  const sizes = bands.map((b) => members.get(b)?.length ?? 0);
  const wide = bands.map((b) => anchors.get(b)?.length ?? 0);
  const ratio = Number.isFinite(aspect) && aspect > 0.2 && aspect < 20 ? aspect : ASPECT;
  const rows = rowsFor(sizes, wide, shelf.length, col, row, ratio);
  const spans = bands.map((_b, i) => spanOf(fanOf(sizes[i], 2 * rows), wide[i]));
  const w = bands.length > 0 ? spans.reduce((a, b) => a + b, 0) * col : Math.max(x1 - x0, h * ratio, 1);
  const left = cx - w / 2;
  const column = new Map<number, number>();
  bands.reduce((x, b, i) => (column.set(b, x + (spans[i] * col) / 2), x + spans[i] * col), left);

  for (const [band, keys] of anchors) {
    const mid = column.get(band)!;
    // More than one anchor in a band: side by side on the line.
    keys.sort().forEach((k, i) => at.set(k, { x: mid + (i - (keys.length - 1) / 2) * col, y: cy }));
  }
  let low = cy - row;
  for (const [band, list] of members) {
    const mid = column.get(band)!;
    const k = fanOf(list.length, 2 * rows);
    // Kept in the order the force layout had them across, so neighbours stay near; then split above and below.
    list.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    const size = Math.ceil(list.length / k);
    for (let j = 0; j < k; j++) {
      const x = mid + (j - (k - 1) / 2 + shiftOf(k)) * col;
      const part = list.slice(j * size, (j + 1) * size).sort((a, b) => b.y - a.y || a.key.localeCompare(b.key));
      const up = Math.ceil(part.length / 2);
      // The upper half nearest the line first going up, the lower half going down, one row apart.
      part.slice(0, up).reverse().forEach((n, i) => at.set(n.key, { x, y: cy + (i + 1) * row }));
      part.slice(up).forEach((n, i) => {
        const y = cy - (i + 1) * row;
        at.set(n.key, { x, y });
        low = Math.min(low, y);
      });
    }
  }
  if (shelf.length > 0) {
    shelf.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    const perRow = Math.max(1, Math.floor(w / col));
    // Below the lane and anything stacked under it: the canvas's y grows upward.
    shelf.forEach((n, i) => at.set(n.key, { x: left + ((i % perRow) + 0.5) * (w / perRow), y: low - 2 * row - Math.floor(i / perRow) * row }));
  }
  const axis =
    bands.length > 0
      ? { graph, y: cy, x0: left, x1: left + w, bands: bands.map((b) => column.get(b)!), anchors: [...anchors.values()].flat().sort(), pitch: row }
      : null;
  return { at, axis, clearance: Math.min(row, col) * 0.45 };
}

/** A fan never borrows another band's x range. When its hub is elsewhere, its members circle a projected centre
 * in their own band; an anchor always keeps its own centre on the axis. */
function spineFans(
  graph: string, nodes: readonly { key: string; id: string; x: number; y: number }[],
  places: ReadonlyMap<string, Place>, hubs: ReadonlyMap<string, string>, row: number, col: number, cx: number, cy: number,
): { at: Map<string, At>; axis: SpineAxis | null; clearance: number } {
  const bands = new Map<number, typeof nodes[number][]>();
  const shelf: typeof nodes[number][] = [];
  for (const n of [...nodes].sort((a, b) => a.key < b.key ? -1 : a.key > b.key ? 1 : 0)) {
    const band = places.get(n.id)?.band;
    if (band === null || band === undefined) shelf.push(n);
    else {
      const list = bands.get(band);
      if (list) list.push(n);
      else bands.set(band, [n]);
    }
  }
  const at = new Map<string, At>(), centres: number[] = [], anchors: string[] = [];
  const pitch = Math.max(row, col);
  let right = 0, bottom = 0;
  for (const [, list] of [...bands].sort(([a], [b]) => a - b)) {
    const groups = new Map<string, typeof list>();
    const free: typeof list = [];
    for (const n of list) {
      const head = places.get(n.id)?.anchor ? n.key : hubs.get(n.key);
      if (!head) free.push(n);
      else {
        const group = groups.get(head);
        if (group) group.push(n);
        else groups.set(head, [n]);
      }
    }
    const start = right;
    for (const [head, members] of [...groups].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) {
      const children = members.filter((n) => n.key !== head);
      const offsets: At[] = [];
      for (let ring = 1; offsets.length < children.length; ring++) {
        const radius = ring * pitch;
        const count = Math.min(children.length - offsets.length, Math.max(1, Math.floor(Math.PI / Math.asin(0.5 / ring))));
        for (let i = 0; i < count; i++) offsets.push({ x: Math.cos(i * 2 * Math.PI / count) * radius, y: Math.sin(i * 2 * Math.PI / count) * radius });
      }
      const reach = offsets.reduce((m, p) => Math.max(m, Math.hypot(p.x, p.y)), 0) + pitch / 2;
      const x = right + reach;
      for (const n of members) if (n.key === head) at.set(n.key, { x, y: 0 });
      children.forEach((n, i) => at.set(n.key, { x: x + offsets[i].x, y: offsets[i].y }));
      bottom = Math.min(bottom, -reach);
      right += reach * 2 + pitch;
    }
    const columns = Math.max(1, Math.ceil(Math.sqrt(free.length)));
    free.forEach((n, i) => {
      const y = (Math.floor(i / columns) - Math.floor(free.length / columns) / 2) * pitch;
      at.set(n.key, { x: right + ((i % columns) + 0.5) * pitch, y });
      bottom = Math.min(bottom, y);
    });
    if (free.length > 0) right += columns * pitch + pitch;
    centres.push((start + right) / 2);
    for (const n of list) if (places.get(n.id)?.anchor) anchors.push(n.key);
  }
  const width = Math.max(right, Math.ceil(Math.sqrt(shelf.length)) * pitch, pitch);
  const shelfColumns = Math.max(1, Math.floor(width / pitch));
  shelf.forEach((n, i) => at.set(n.key, { x: ((i % shelfColumns) + 0.5) * pitch, y: bottom - 2 * pitch - Math.floor(i / shelfColumns) * pitch }));
  for (const p of at.values()) { p.x += cx - width / 2; p.y += cy; }
  return {
    at, clearance: Math.min(row, col) * 0.45,
    axis: centres.length > 0 ? { graph, y: cy, x0: cx - width / 2, x1: cx + width / 2, bands: centres.map((x) => x + cx - width / 2), anchors, pitch: row } : null,
  };
}

/** An anchor's label narrower than this (px) is not cut to fit before the next anchor: it runs past it, to at most
 * ANCHOR_LABEL_MAX, and the anchors it passes go unlabelled. */
export const ANCHOR_LABEL_MIN = 40;
export const ANCHOR_LABEL_MAX = 180;

/**
 * The width (px) each anchor's label is drawn at, left to right along each line; an anchor left out gets no label.
 * `anchors` are on screen: `x`, `y` the centre, `size` the drawn half-width, `width` the whole label's width. A label
 * takes the room up to the next anchor on its line (`y` within a label's height) less `gap`; with less room than
 * ANCHOR_LABEL_MIN it runs on, cut to ANCHOR_LABEL_MAX, and every anchor whose label would start under it is left out.
 * No two drawn labels meet.
 */
export function anchorLabels(anchors: readonly { key: string; x: number; y: number; size: number; width: number }[], labelPx: number, gap = 8): Map<string, number> {
  const out = new Map<string, number>();
  const sorted = [...anchors].sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
  const ends: { y: number; x: number }[] = [];
  sorted.forEach((a, i) => {
    const start = a.x + a.size + 3;
    const end = ends.find((e) => Math.abs(e.y - a.y) < labelPx);
    if (end && start < end.x + gap) return;
    let j = i + 1;
    while (j < sorted.length && Math.abs(sorted[j].y - a.y) >= labelPx) j++;
    const room = j < sorted.length ? sorted[j].x - sorted[j].size - start - gap : Infinity;
    const width = Math.min(a.width, room >= ANCHOR_LABEL_MIN ? room : ANCHOR_LABEL_MAX);
    out.set(a.key, width);
    if (end) end.x = start + width;
    else ends.push({ y: a.y, x: start + width });
  });
  return out;
}

/** Whether a spine's rows are far enough apart on screen (`pitchPx`) for each to carry a label `labelPx` high. */
export const rowsLabelled = (pitchPx: number, labelPx: number) => pitchPx >= labelPx + 2;

/** The spine a graph shows: the person's pick (null for none), else its main spine, else its first. */
export function shownSpine(spines: readonly { name: string; main: boolean }[] | undefined, picked: string | null | undefined): string | null {
  if (picked === null) return null;
  if (picked !== undefined && spines?.some((s) => s.name === picked)) return picked;
  return (spines?.find((s) => s.main) ?? spines?.[0])?.name ?? null;
}

/** A spine's toggle, in the sidebar or the node panel: picking the shown spine puts it away (none), any other shows it. */
export function toggleSpine(shown: string | null, name: string): string | null {
  return shown === name ? null : name;
}

/** Where one spine's placements stand for a graph: read, being read, or why not. */
export type SpineState = "ready" | "loading" | "unsupported" | "failed" | "gone";

/** How one node sits on one spine, for the node panel: its place (null while unread or not on the spine) and the
 * anchors of its band (umx ids), and where its band falls when no anchor is in it. */
export type NodeOnSpine = {
  name: string;
  title: string;
  clock: "calendar" | "order";
  main: boolean;
  shown: boolean;
  state: SpineState;
  place: Place | null;
  anchors: string[];
  side: "before" | "after" | "between" | null;
};

/**
 * How node `id` sits on each declared spine of its graph, in the order the graph lists them. `read(name)` gives a
 * spine's state and, once read, its placements; `shown` is the spine the graph shows now (null for none).
 */
export function nodeOnSpines(
  spines: readonly { name: string; title: string; clock: "calendar" | "order"; main: boolean }[],
  id: string,
  shown: string | null,
  read: (name: string) => { state: SpineState; places: ReadonlyMap<string, Place> | null },
): NodeOnSpine[] {
  return spines.map((s) => {
    const { state, places } = read(s.name);
    const p = state === "ready" ? places?.get(id) : undefined;
    const place = p && p.band !== null ? p : null;
    const anchors: string[] = [];
    let side: NodeOnSpine["side"] = null;
    if (place && places) {
      let [lo, hi] = [Infinity, -Infinity];
      for (const [k, q] of places) {
        if (!q.anchor || q.band === null) continue;
        lo = Math.min(lo, q.band);
        hi = Math.max(hi, q.band);
        if (q.band === place.band && k !== id) anchors.push(k);
      }
      if (!place.anchor && anchors.length === 0 && lo <= hi) side = place.band! < lo ? "before" : place.band! > hi ? "after" : "between";
      anchors.sort();
    }
    return { name: s.name, title: s.title, clock: s.clock, main: s.main, shown: shown === s.name, state, place, anchors, side };
  });
}

/** One spine's placements as read for an open graph: `at` is that graph's live change count it was read at. */
export type SpineRead = { at: number; loading: boolean; places: Map<string, Place> | null; supported: boolean; failed?: boolean; gone?: boolean };

/** The spine reads to start now: each wanted [graph, spine] not read yet, or read before its own graph's latest live
 * change; never one already in flight. A change in another open graph leaves a graph's reads alone. */
export function spineReadsDue(wanted: readonly (readonly [string, string])[], reads: ReadonlyMap<string, SpineRead>, changes: ReadonlyMap<string, number>): [string, string][] {
  return wanted.filter(([a, name]) => {
    const had = reads.get(`${a}/${name}`);
    return !had || (!had.loading && had.at !== (changes.get(a) ?? 0));
  }) as [string, string][];
}

// The spine view (Jordan's brief part 2): a graph laid out along one of its declared spines. umx places each node
// (`node list --columns spine:<name>`, its one placement rule); this only turns those bands into positions. Earliest
// band on the left; each anchor on the centre line in its band; the nodes placed in a band beside it, above and below
// the line as the force layout had them; nodes the spine does not place on a shelf under the lane (the canvas's y grows
// upward). The lane keeps the
// graph's own region, so several graphs each keep theirs.

/** Where umx placed one node: its band (null when not placed), how, and whether it is one of the spine's anchors. */
export type Place = { band: number | null; by: string; anchor: boolean };

/** One graph's spine line, in graph coordinates, for the canvas to draw: from the first band to the last. */
export type SpineAxis = { graph: string; y: number; x0: number; x1: number; bands: number[] };

type At = { x: number; y: number };

/** The lane's width is at least this many times its height, so a spine reads left to right. */
const MIN_ASPECT = 1.6;
/** At spread 1, the least gap between two nodes of a band is the force layout's height over this. */
const ROWS = 24;
/** Sub-columns of a band sit this many gaps apart at spread 1, room for a short label. */
const FAN = 2.5;

/** How many sub-columns a band of `n` nodes fans into at spread `s`: more as either grows, and an even number past one
 * node, so none sits on the anchor's own column and its edges to the anchor do not run along each other. */
export function fanOf(n: number, s: number): number {
  if (n <= 1) return n;
  const k = Math.ceil(Math.sqrt((n * s) / 2));
  return k + (k % 2);
}

/** The Spread slider (spine view): it widens the gap between bands and between the nodes of a band. */
export const SPREAD = { min: 0.5, max: 4, step: 0.1, default: 1 } as const;

/** A spread in range, else the default. */
export function spreadOf(v: number | undefined): number {
  return v !== undefined && Number.isFinite(v) && v >= SPREAD.min && v <= SPREAD.max ? v : SPREAD.default;
}

/**
 * Ys for one side of the line, nearest the line first: each where the force layout had it, pushed out from `from`
 * (the line) and from the node before it so no two sit closer than `gap`. `dir` is 1 above the line, -1 below.
 */
export function stack(ys: readonly number[], from: number, gap: number, dir: 1 | -1): number[] {
  let prev = from;
  return ys.map((y) => (prev = dir > 0 ? Math.max(y, prev + gap) : Math.min(y, prev - gap)));
}

/**
 * Positions for one graph's nodes along a spine. `nodes` are the graph's nodes with their force-layout positions
 * (`key` the canvas key, `id` umx's id); `places` umx's placements by id; `spread` the Spread slider. Returns each
 * node's position and the axis. A band fans into sub-columns around its anchor, kept in the force layout's order across,
 * and no two of its nodes sit closer than the gap. A wider spread moves a band's nodes further off the line, apart from
 * each other, and fans each band wider.
 */
export function spineLayout(graph: string, nodes: readonly { key: string; id: string; x: number; y: number }[], places: ReadonlyMap<string, Place>, spread: number = SPREAD.default): { at: Map<string, At>; axis: SpineAxis | null } {
  const at = new Map<string, At>();
  if (nodes.length === 0) return { at, axis: null };
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
  // Spread pushes a band's nodes off the line by s and its bands and sub-columns apart by sqrt(s): the lane grows
  // taller faster than wider, so at Fit a wide screen shows a wider spread bigger, not squashed.
  const gap = (h / ROWS) * s;
  const step = (h / ROWS) * FAN * Math.sqrt(s);

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
  const fans = new Map(bands.map((b) => [b, fanOf(members.get(b)?.length ?? 0, s)]));
  const widest = Math.max(1, ...fans.values(), ...bands.map((b) => anchors.get(b)?.length ?? 0));
  const w0 = Math.max(x1 - x0, h * MIN_ASPECT, 1);
  const col = Math.max(bands.length > 0 ? (w0 / bands.length) * Math.sqrt(s) : w0, (widest + 1) * step);
  const w = bands.length > 0 ? col * bands.length : w0;
  const left = cx - w / 2;
  const column = new Map(bands.map((b, i) => [b, left + (i + 0.5) * col]));

  for (const [band, keys] of anchors) {
    const mid = column.get(band)!;
    // More than one anchor in a band: side by side on the line.
    keys.sort().forEach((k, i) => at.set(k, { x: mid + (i - (keys.length - 1) / 2) * step, y: cy }));
  }
  let low = y0;
  for (const [band, list] of members) {
    const mid = column.get(band)!;
    const k = fans.get(band)!;
    // Kept in the order the force layout had them across, so neighbours stay near; then stacked off the line.
    list.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    const size = Math.ceil(list.length / k);
    for (let j = 0; j < k; j++) {
      const x = mid + (j - (k - 1) / 2) * step;
      const part = list.slice(j * size, (j + 1) * size);
      for (const dir of [1, -1] as const) {
        const side = part.filter((n) => (dir > 0 ? n.y >= cy : n.y < cy)).sort((a, b) => dir * (a.y - b.y) || a.key.localeCompare(b.key));
        const ys = stack(
          side.map((n) => cy + (n.y - cy) * s),
          cy,
          gap,
          dir,
        );
        side.forEach((n, i) => {
          at.set(n.key, { x, y: ys[i] });
          low = Math.min(low, ys[i]);
        });
      }
    }
  }
  if (shelf.length > 0) {
    shelf.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    const perRow = Math.max(1, Math.min(Math.ceil(Math.sqrt(shelf.length * MIN_ASPECT * 4)), Math.floor(w / step)));
    const row = Math.max(h * 0.06, gap);
    // Below the lane and anything stacked under it: the canvas's y grows upward.
    shelf.forEach((n, i) => at.set(n.key, { x: left + ((i % perRow) + 0.5) * (w / perRow), y: low - h * 0.15 - Math.floor(i / perRow) * row }));
  }
  const axis = bands.length > 0 ? { graph, y: cy, x0: left, x1: left + w, bands: bands.map((b) => column.get(b)!) } : null;
  return { at, axis };
}

/** The spine a graph shows: the person's pick (null for none), else its main spine, else its first. */
export function shownSpine(spines: readonly { name: string; main: boolean }[] | undefined, picked: string | null | undefined): string | null {
  if (picked === null) return null;
  if (picked !== undefined && spines?.some((s) => s.name === picked)) return picked;
  return (spines?.find((s) => s.main) ?? spines?.[0])?.name ?? null;
}

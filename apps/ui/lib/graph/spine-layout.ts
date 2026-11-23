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
/** A band's nodes spread over this share of its column. */
const SPREAD = 0.8;

/**
 * Positions for one graph's nodes along a spine. `nodes` are the graph's nodes with their force-layout positions
 * (`key` the canvas key, `id` umx's id); `places` umx's placements by id. Returns each node's position and the axis.
 */
export function spineLayout(graph: string, nodes: readonly { key: string; id: string; x: number; y: number }[], places: ReadonlyMap<string, Place>): { at: Map<string, At>; axis: SpineAxis | null } {
  const at = new Map<string, At>();
  if (nodes.length === 0) return { at, axis: null };
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
  const w = Math.max(x1 - x0, h * MIN_ASPECT, 1);
  const left = cx - w / 2;

  const bands = [...new Set(nodes.map((n) => places.get(n.id)?.band).filter((b): b is number => typeof b === "number"))].sort((a, b) => a - b);
  const col = bands.length > 0 ? w / bands.length : w;
  const column = new Map(bands.map((b, i) => [b, left + (i + 0.5) * col]));

  const members = new Map<number, { key: string; x: number; y: number }[]>();
  const shelf: { key: string; x: number }[] = [];
  for (const n of nodes) {
    const p = places.get(n.id);
    if (!p || p.band === null) {
      shelf.push({ key: n.key, x: n.x });
    } else if (p.anchor) {
      at.set(n.key, { x: column.get(p.band)!, y: cy });
    } else {
      const list = members.get(p.band) ?? [];
      list.push(n);
      members.set(p.band, list);
    }
  }
  for (const [band, list] of members) {
    const mid = column.get(band)!;
    // Kept in the order the force layout had them across, so neighbours stay near.
    list.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    list.forEach((n, i) => at.set(n.key, { x: mid - (col * SPREAD) / 2 + (col * SPREAD * (i + 0.5)) / list.length, y: n.y }));
  }
  if (shelf.length > 0) {
    shelf.sort((a, b) => a.x - b.x || a.key.localeCompare(b.key));
    const perRow = Math.max(1, Math.ceil(Math.sqrt(shelf.length * MIN_ASPECT * 4)));
    const row = h * 0.06;
    // Below the lane: the canvas's y grows upward.
    shelf.forEach((n, i) => at.set(n.key, { x: left + ((i % perRow) + 0.5) * (w / perRow), y: y0 - h * 0.15 - Math.floor(i / perRow) * row }));
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

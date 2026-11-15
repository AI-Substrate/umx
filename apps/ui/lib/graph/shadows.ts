// Shadow connections (ws009 M1c, plan 169): pairs umx finds similar that nobody linked, inside an open graph and across
// open graphs, drawn as dotted lines. Pure: the pairs from umx's rows, which are allowed on the canvas, and which the
// Selected and All modes draw. A pair is undirected, once, at the higher of its two directions' scores.
import type { ViewGraph } from "@/lib/graph/build";
import type { Entities, Place } from "@/lib/graph/entities";
import { qid } from "@/lib/graph/keys";
import { aliasOfKey } from "@/lib/graph/keys";
import type { ShadowGroup, ShadowRow } from "@/lib/shadows-read";

export type ShadowMode = "off" | "selected" | "all";
/** Canvas keys, `a` < `b`, the score, and its model's floor (null: ranked only). */
export type Pair = { a: string; b: string; score: number; floor: number | null };
export type ShadowIndex = { pairs: Pair[]; byNode: Map<string, Pair[]> };

/** Selected: the selected node's top 5, and a hovered node's top 3. All: each shown node's best, at most 600 lines
 * (ui-expert N1: past that the canvas reads as noise; the rest are counted). */
export const SELECTED_TOP = 5;
export const HOVER_TOP = 3;
export const ALL_CAP = 600;
/** The most rows one read takes (12 graphs of 2,000 nodes at k 10 is 240,000). */
export const SHADOW_ROWS_MAX = 300_000;

const byScore = (x: Pair, y: Pair) => y.score - x.score || (x.a < y.a ? -1 : x.a > y.a ? 1 : x.b < y.b ? -1 : x.b > y.b ? 1 : 0);

/** umx's directed rows as undirected pairs, each once at the higher score, best first, with their model's floor. */
export function pairsOf(rows: readonly ShadowRow[], groups: readonly Pick<ShadowGroup, "floor">[] = []): Pair[] {
  const best = new Map<string, Pair>();
  for (const [ga, ia, gb, ib, score, group] of rows) {
    const [x, y] = [qid(ga, ia), qid(gb, ib)];
    if (x === y) continue;
    const [a, b] = x < y ? [x, y] : [y, x];
    const k = `${a}\u0000${b}`;
    const was = best.get(k);
    if (!was || score > was.score) best.set(k, { a, b, score, floor: groups[group]?.floor ?? null });
  }
  return [...best.values()].sort(byScore);
}

/** A new read of `read` (graphs) over what was drawn: its pairs replace every old pair between two of those graphs;
 * an old pair with an end in a graph not read again (one whose changes are still held, review B3) stays. */
export function mergeShadowReads(old: readonly Pair[], fresh: readonly Pair[], read: ReadonlySet<string>): Pair[] {
  const kept = old.filter((p) => !read.has(aliasOfKey(p.a)) || !read.has(aliasOfKey(p.b)));
  return [...kept, ...fresh].sort(byScore);
}

export function indexPairs(input: readonly Pair[]): ShadowIndex {
  const pairs = [...input].sort(byScore);
  const byNode = new Map<string, Pair[]>();
  for (const p of pairs) {
    for (const n of [p.a, p.b]) {
      const list = byNode.get(n);
      if (list) list.push(p);
      else byNode.set(n, [p]);
    }
  }
  return { pairs, byNode };
}

/** What a shadow may be drawn between: both ends on the canvas and shown, no real edge between them either way, and
 * not two members of one entity. `shown` is this render's visibility (the filters as they are now), never the node's
 * hidden attribute, which the canvas updates only after (review B1). */
export type ShadowCtx = { graph: ViewGraph; entities: Entities; shown: (key: string) => boolean };

/** `shown` from a visibility mask by node index (build.ts shownMask). */
export function shownBy(graph: ViewGraph, mask: Uint8Array | null): (key: string) => boolean {
  return (key) => graph.hasNode(key) && (mask ? mask[graph.getNodeAttribute(key, "index")] === 1 : !graph.getNodeAttribute(key, "hidden"));
}

export function allowed(p: Pair, { graph, entities, shown }: ShadowCtx): boolean {
  if (!graph.hasNode(p.a) || !graph.hasNode(p.b)) return false;
  if (!shown(p.a) || !shown(p.b)) return false;
  if (graph.areNeighbors(p.a, p.b)) return false;
  const [ea, eb] = [entities.of.get(p.a), entities.of.get(p.b)];
  return !(ea !== undefined && ea === eb);
}

/** The members a merged node stands for (just the node when it is not merged). */
function membersOf(places: ReadonlyMap<string, Place> | null): (key: string) => string[] {
  if (!places || places.size === 0) return (k) => [k];
  const byEntity = new Map<number, string[]>();
  for (const [k, p] of places) byEntity.set(p.entity, [...(byEntity.get(p.entity) ?? []), k]);
  return (k) => {
    const p = places.get(k);
    return p ? byEntity.get(p.entity)! : [k];
  };
}

/** Allowed, and while merged also not between two merged nodes that a real edge of any of their members already
 * joins, nor within one merged node (review B5). */
function eligible(ctx: ShadowCtx, places: ReadonlyMap<string, Place> | null) {
  const members = membersOf(places);
  return (p: Pair) => {
    if (!allowed(p, ctx)) return false;
    if (!places || places.size === 0) return true;
    const [ma, mb] = [members(p.a), members(p.b)];
    if (ma.includes(p.b)) return false;
    return !ma.some((x) => mb.some((y) => ctx.graph.areNeighbors(x, y)));
  };
}

const other = (p: Pair, n: string) => (p.a === n ? p.b : p.a);

/** Merged entities: each end as its entity's merged node (its head), the pairs deduplicated by those ends at the
 * higher score, and none from a merged node to itself. */
export function mergedPairs(pairs: readonly Pair[], places: ReadonlyMap<string, Place> | null): Pair[] {
  if (!places || places.size === 0) return [...pairs];
  const heads = new Map<number, string>();
  for (const [k, p] of places) if (p.head) heads.set(p.entity, k);
  const rep = (k: string) => {
    const p = places.get(k);
    return p ? (heads.get(p.entity) ?? k) : k;
  };
  const best = new Map<string, Pair>();
  for (const p of pairs) {
    const [x, y] = [rep(p.a), rep(p.b)];
    if (x === y) continue;
    const [a, b] = x < y ? [x, y] : [y, x];
    const k = `${a}\u0000${b}`;
    const was = best.get(k);
    if (!was || p.score > was.score) best.set(k, { a, b, score: p.score, floor: p.floor });
  }
  return [...best.values()].sort(byScore);
}

/** A node's own top `n` allowed shadows; for a member of a merged entity, its members' together. */
export function nodeShadows(index: ShadowIndex, ctx: ShadowCtx, node: string, n: number, places: ReadonlyMap<string, Place> | null = null): Pair[] {
  const place = places?.get(node);
  const members = place ? [...places!].filter(([, p]) => p.entity === place.entity).map(([k]) => k) : [node];
  const ok = eligible(ctx, places);
  const mine = members.flatMap((m) => (index.byNode.get(m) ?? []).filter(ok));
  return mergedPairs(mine, places).slice(0, n);
}

/** All: each shown node's best eligible shadow, each pair once, best first, at most `cap`; `more` counts the rest. */
export function allShadows(index: ShadowIndex, ctx: ShadowCtx, places: ReadonlyMap<string, Place> | null = null, cap = ALL_CAP): { pairs: Pair[]; more: number } {
  const picked = new Map<string, Pair>();
  const ok = eligible(ctx, places);
  for (const [node, list] of index.byNode) {
    if (!ctx.shown(node)) continue;
    const best = list.find(ok);
    if (best) picked.set(`${best.a}\u0000${best.b}`, best);
  }
  const all = mergedPairs([...picked.values()], places);
  return { pairs: all.slice(0, cap), more: Math.max(0, all.length - cap) };
}

/** A shadow's opacity: 0.35 at the floor to 0.8 at 1.0. Uncalibrated (no floor): ranked only, one middle opacity. */
export function shadowAlpha(score: number, floor: number | null): number {
  if (floor === null || floor >= 1) return 0.5;
  const t = Math.min(1, Math.max(0, (score - floor) / (1 - floor)));
  return 0.35 + 0.45 * t;
}

/** The far end of a pair from `node`; for a member of a merged entity (whose pairs name its head), the end outside it. */
export function farEnd(p: Pair, node: string, places: ReadonlyMap<string, Place> | null = null): string {
  if (p.a === node || p.b === node) return other(p, node);
  const mine = places?.get(node)?.entity;
  return mine !== undefined && places?.get(p.a)?.entity === mine ? p.b : p.a;
}

/** Shadow data belongs to the open set it was read over (security's scope rules on M1c, as review C1): one read over
 * another set is never drawn or listed. Hold keeps the applied state's shadows instead (review B3). */
export function shadowScope(viewKey: string | null): string | null {
  return viewKey;
}

export function forScope<T extends { scope: string }>(held: T | null, scope: string | null): T | null {
  return held && scope !== null && held.scope === scope ? held : null;
}

/** Graphs umx did not compare, grouped by reason (ui-expert N2): one legend line for them all, and one panel
 * line per reason, never one per graph. */
function byReason(skipped: readonly { graph: string; reason: string }[]): [string, string[]][] {
  const out = new Map<string, string[]>();
  for (const s of skipped) out.set(s.reason, [...(out.get(s.reason) ?? []), s.graph]);
  return [...out];
}
export function skippedLegend(skipped: readonly { graph: string; reason: string }[]): string | null {
  if (skipped.length === 0) return null;
  const n = skipped.length;
  return `${n} ${n === 1 ? "graph" : "graphs"} not compared: ${byReason(skipped).map(([reason, gs]) => `${reason} (${gs.join(", ")})`).join("; ")}`;
}
export function skippedPanel(skipped: readonly { graph: string; reason: string }[]): string[] {
  return byReason(skipped).map(([reason, gs]) => (gs.length === 1 ? `${gs[0]} has ${reason}; no shadows to it.` : `${gs.join(", ")} have ${reason}; no shadows to them.`));
}

// Entities across the open graphs (ws009 M2, plan 165; denials M2b, plan 176), built with umx's own rule
// (umx-services graph_store/entity.rs, `umx entity show`): nodes of one type that share an entity_key, or are joined by
// a same_as edge, followed onward; possibly_same_as partners are proposed, never joined. A not_same_as edge (a denial,
// ws008 E2c) splits: the joins run again in umx's fixed order (keys by type and key, each key's holders by graph id and
// node id, then same_as links by storing graph id and edge id), and a join that would put a denied pair in one group is
// skipped. A denial counts only while its storing graph and both its ends are covered. Only the open graphs are
// covered, one copy per graph id (the first open), as umx covers one copy per graph id (d-21). A node is named by its
// graph id and id (`At`), so a link into a graph that is not open still joins the walk through the links the open
// graphs store to it, and is never shown, as umx does.
import type { ViewGraph } from "@/lib/graph/build";
import { aliasOfKey, qid, unq } from "@/lib/graph/keys";
import type { Snapshot } from "@/lib/types";

export const SAME_AS = "same_as";
export const POSSIBLY_SAME_AS = "possibly_same_as";
export const NOT_SAME_AS = "not_same_as";
/** The most keys umx reads from one node (umx-core entity MAX_KEYS). */
export const MAX_KEYS = 32;
/** What the view shows wherever a key umx flags as a possible secret would be (ws002 F188; security on M2 and M2b). */
export const HIDDEN_KEY = "(hidden key)";
/** What a flagged field, evidence line or reason shows in its place (lib/secret-flags replaces them on the server). */
export const HIDDEN_VALUE = "(hidden: may be a secret)";

const SCHEME = /^[a-z][a-z0-9_]{0,31}$/;
/** A key's scheme as the panel may show it (security): only a plain scheme, else "(hidden key)". */
export function keyShown(key: string): string {
  const i = key.indexOf(":");
  const scheme = i > 0 ? key.slice(0, i) : "";
  return SCHEME.test(scheme) ? scheme : HIDDEN_KEY;
}

/** A key umx flags (`key:<i>`), as the panel shows it: its plain scheme and "(hidden key)", never the value. */
export function hiddenKeyText(key: string): string {
  const scheme = keyShown(key);
  return scheme === HIDDEN_KEY ? HIDDEN_KEY : `${scheme}:${HIDDEN_KEY}`;
}

/** A node as the walk meets it: `<graph id>/<node id>`. */
export type At = string;

export type IdentityKind = typeof SAME_AS | typeof POSSIBLY_SAME_AS | typeof NOT_SAME_AS;
/** A stored identity edge: its canvas key (`<storing alias>/<id>`), the storing alias, its kind and both ends. */
export type Link = { edge: string; owner: string; kind: IdentityKind; from: At; to: At };

type Held = { key: string; kind: string; keys: string[] };

/** What the walk reads: the covered nodes by At, the key index, and the identity edges by end. */
export type World = {
  /** Covered graphs: graph id -> the alias of its first open copy. */
  covered: ReadonlyMap<string, string>;
  nodes: ReadonlyMap<At, Held>;
  /** `<graph id>\0<type>\0<key>` -> the one node holding it there, or null when several do: a key two nodes of one
   * type hold in their own graph joins neither, anywhere (ws008 E2b, entity_key_ignored). */
  keyIndex: ReadonlyMap<string, string | null>;
  links: ReadonlyMap<At, Link[]>;
};

const IDENTITY = new Set<string>([SAME_AS, POSSIBLY_SAME_AS, NOT_SAME_AS]);
const isIdentity = (kind: string): kind is IdentityKind => IDENTITY.has(kind);

/** A node's keys as umx reads them: strings, each once, at most MAX_KEYS. */
export function keysOf(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const out: string[] = [];
  for (const k of value) {
    if (out.length === MAX_KEYS) break;
    if (typeof k === "string" && k !== "" && !out.includes(k)) out.push(k);
  }
  return out;
}

/** The first open alias of each graph id, in open order. */
export function coveredOf(snaps: readonly Pick<Snapshot, "graph" | "graphId">[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const s of snaps) if (!out.has(s.graphId)) out.set(s.graphId, s.graph);
  return out;
}

/** The identity edges the snapshots store that are not on the canvas: an end in a graph that is not open, or open in
 * several copies. The walk follows them as umx does; they are never drawn. */
export function throughLinks(snaps: readonly Snapshot[], graph: ViewGraph): Link[] {
  const out: Link[] = [];
  for (const s of snaps) {
    for (const [id, kind, fromGraph, from, toGraph, to] of s.external ?? []) {
      const edge = qid(s.graph, id);
      if (!isIdentity(kind) || graph.hasEdge(edge)) continue;
      out.push({ edge, owner: s.graph, kind, from: `${fromGraph}/${from}`, to: `${toGraph}/${to}` });
    }
  }
  return out;
}

/** The walk's world from what is on the canvas: covered nodes, the key index, and the identity edges covered graphs
 * store (drawn ones and `through`). */
export function worldOf(graph: ViewGraph, graphIds: ReadonlyMap<string, string>, covered: ReadonlyMap<string, string>, through: readonly Link[] = []): World {
  const nodes = new Map<At, Held>();
  const keyIndex = new Map<string, string | null>();
  const isCovered = (alias: string) => {
    const gid = graphIds.get(alias);
    return gid !== undefined && covered.get(gid) === alias;
  };
  graph.forEachNode((key, a) => {
    if (!isCovered(a.graph)) return;
    const { id } = unq(key);
    const gid = graphIds.get(a.graph)!;
    const keys = a.keys ?? [];
    nodes.set(`${gid}/${id}`, { key, kind: a.kind, keys });
    for (const k of keys) {
      const ik = `${gid}\0${a.kind}\0${k}`;
      keyIndex.set(ik, keyIndex.has(ik) ? null : id);
    }
  });
  const links = new Map<At, Link[]>();
  const seen = new Set<string>();
  const add = (l: Link) => {
    if (seen.has(l.edge) || !isCovered(l.owner)) return;
    seen.add(l.edge);
    for (const end of l.from === l.to ? [l.from] : [l.from, l.to]) {
      const list = links.get(end);
      if (list) list.push(l);
      else links.set(end, [l]);
    }
  };
  const at = (key: string) => `${graphIds.get(aliasOfKey(key))}/${unq(key).id}`;
  graph.forEachEdge((edge, a, s, t) => {
    if (isIdentity(a.kind)) add({ edge, owner: aliasOfKey(edge), kind: a.kind, from: at(s), to: at(t) });
  });
  for (const l of through) add(l);
  return { covered, nodes, keyIndex, links };
}

/** Whether node `n` (at `at`) joins by key `k`: it is the only node of its type holding `k` in its own graph. */
function usable(world: World, at: At, n: Pick<Held, "kind">, k: string): boolean {
  return world.keyIndex.get(`${gidOf(at)}\0${n.kind}\0${k}`) !== null;
}

const gidOf = (at: At) => at.slice(0, at.indexOf("/"));
const idOf = (at: At) => at.slice(at.indexOf("/") + 1);

/** Rust's String order (bytes of UTF-8), which is code point order; JS's `<` compares UTF-16 units. */
export function byCodePoint(a: string, b: string): number {
  const [x, y] = [[...a], [...b]];
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const d = x[i].codePointAt(0)! - y[i].codePointAt(0)!;
    if (d !== 0) return d;
  }
  return x.length - y.length;
}
const cmp = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
/** umx's At order: graph id, then node id (both 128-bit ids, whose ULID text sorts as the number). */
export const byAt = (a: At, b: At) => cmp(gidOf(a), gidOf(b)) || cmp(idOf(a), idOf(b));

/** umx-core's reserved words: a key whose scheme is one never anchors a join (ValidName::new). */
const RESERVED = new Set(
  ("add all alter and any as asc ascending attach by call case cast column contains copy count create default delete " +
    "desc descending detach distinct drop else end ends exists export false from glob group import in install is key " +
    "limit load macro match merge node none not null on optional or order primary profile recursive rel remove rename " +
    "return set shortest single skip starts table then to true union unwind use when where with xor yield").split(" "),
);
/** Whether a key's scheme is a valid name (snake_case, at most 64, not reserved), as umx's join needs. */
function joinable(key: string): boolean {
  const i = key.indexOf(":");
  if (i < 0) return false;
  const scheme = key.slice(0, i);
  return /^[a-z][a-z0-9_]*$/.test(scheme) && scheme.length <= 64 && !RESERVED.has(scheme);
}

/** The groups a set of reached nodes falls into once denials split them (umx's `group_of`, ws008 E2c): `root` of each
 * reached node, the members of each root, and each root's denied partners (both ends covered). */
export type Grouped = { root: Map<At, At>; members: Map<At, Set<At>>; denied: Map<At, Set<At>>; pairs: Map<At, Set<At>> };

export function groupOf(world: World, reached: Iterable<At>, met: readonly Link[]): Grouped {
  const parent = new Map<At, At>();
  const members = new Map<At, Set<At>>();
  for (const at of reached) {
    parent.set(at, at);
    members.set(at, new Set([at]));
  }
  // A denial counts only while its graph and both its ends are covered (security on E2c). Every link in the world is
  // stored in a covered graph.
  const pairs = new Map<At, Set<At>>();
  for (const m of met) {
    if (m.kind !== NOT_SAME_AS || !world.covered.has(gidOf(m.from)) || !world.covered.has(gidOf(m.to))) continue;
    for (const [p, q] of [[m.from, m.to], [m.to, m.from]]) {
      const list = pairs.get(p);
      if (list) list.add(q);
      else pairs.set(p, new Set([q]));
    }
  }
  const find = (x: At): At => {
    let r = x;
    while (parent.get(r) !== r) r = parent.get(r)!;
    let c = x;
    while (parent.get(c) !== r) {
      const n = parent.get(c)!;
      parent.set(c, r);
      c = n;
    }
    return r;
  };
  // The joins, in their fixed order: keys by (type, stored key), each key's single holders from the first.
  const joins: [At, At][] = [];
  const byKey = new Map<string, { kind: string; key: string; holders: At[] }>();
  for (const at of parent.keys()) {
    const n = world.nodes.get(at);
    if (!n) continue;
    for (const key of n.keys) {
      const ik = `${n.kind}\0${key}`;
      const entry = byKey.get(ik);
      if (entry) entry.holders.push(at);
      else byKey.set(ik, { kind: n.kind, key, holders: [at] });
    }
  }
  const keyed = [...byKey.values()].sort((a, b) => byCodePoint(a.kind, b.kind) || byCodePoint(a.key, b.key));
  for (const { kind, key, holders } of keyed) {
    if (!joinable(key)) continue;
    const single = holders.filter((h) => usable(world, h, { kind }, key)).sort(byAt);
    for (const h of single.slice(1)) joins.push([single[0], h]);
  }
  const gidOfAlias = new Map([...world.covered].map(([g, a]) => [a, g]));
  const links = met.filter((m) => m.kind === SAME_AS);
  links.sort((a, b) => cmp(gidOfAlias.get(a.owner) ?? "", gidOfAlias.get(b.owner) ?? "") || cmp(unq(a.edge).id, unq(b.edge).id));
  for (const m of links) joins.push([m.from, m.to]);
  for (const [x, y] of joins) {
    if (!parent.has(x) || !parent.has(y)) continue;
    const [rx, ry] = [find(x), find(y)];
    if (rx === ry) continue;
    const there = members.get(ry)!;
    let clash = false;
    if (pairs.size > 0) {
      for (const p of members.get(rx)!) {
        for (const q of pairs.get(p) ?? []) if (there.has(q)) clash = true;
        if (clash) break;
      }
    }
    if (clash) continue;
    for (const m of there) members.get(rx)!.add(m);
    members.delete(ry);
    parent.set(ry, rx);
  }
  const root = new Map<At, At>();
  for (const at of parent.keys()) root.set(at, find(at));
  const denied = new Map<At, Set<At>>();
  for (const [at, partners] of pairs) {
    const r = root.get(at);
    if (r === undefined) continue;
    const list = denied.get(r) ?? new Set<At>();
    for (const p of partners) list.add(p);
    denied.set(r, list);
  }
  return { root, members, denied, pairs };
}

/** Whether a proposal from the group rooted at `r` to `other` is contradicted by a denial (umx drops it): `other`, or
 * a node of its group, is one the group is denied with. */
function blocked(g: Grouped, r: At, other: At): boolean {
  const partners = g.denied.get(r);
  if (!partners) return false;
  if (partners.has(other)) return true;
  const ro = g.root.get(other);
  if (ro === undefined) return false;
  for (const p of partners) if (g.root.get(p) === ro) return true;
  return false;
}

/** How a member joined: the node asked about, a key it shares with a member of its type, or a same_as edge (stored
 * in `graph`). */
export type Match = { by: "asked" } | { by: "key"; key: string } | { by: "link"; graph: string; edge: string };

export type Member = { at: At; key: string; graph: string; kind: string; matched: Match[] };
export type Proposed = { at: At; key: string; graph: string; with: string; edge: string; graph_of_edge: string };
/** A denial that counts between a member and an open node outside the entity: names of open nodes only. */
export type Denied = { at: At; key: string; graph: string; with: string; edge: string; graph_of_edge: string };
export type Walked = { members: Member[]; proposed: Proposed[]; denied: Denied[] };

/** Every node a key or a same_as link reaches from `first`, and the identity edges met on the way (umx's
 * walk_entity): denials do not stop the walk; groupOf splits what it reached. */
function walk(world: World, first: At): { reached: Set<At>; met: Link[] } {
  const reached = new Set<At>([first]);
  const queue: At[] = [first];
  const met: Link[] = [];
  const seenEdges = new Set<string>();
  const covered = [...world.covered.keys()];
  while (queue.length > 0) {
    const at = queue.shift()!;
    const next: At[] = [];
    if (world.covered.has(gidOf(at))) {
      const n = world.nodes.get(at);
      if (!n) continue;
      for (const k of n.keys) {
        if (!usable(world, at, n, k)) continue;
        for (const other of covered) {
          const id = world.keyIndex.get(`${other}\0${n.kind}\0${k}`);
          if (id) next.push(`${other}/${id}`);
        }
      }
    }
    for (const l of world.links.get(at) ?? []) {
      if (seenEdges.has(l.edge)) continue;
      seenEdges.add(l.edge);
      if (l.kind === SAME_AS) next.push(l.from === at ? l.to : l.from);
      met.push(l);
    }
    for (const n of next) {
      if (!reached.has(n)) {
        reached.add(n);
        queue.push(n);
      }
    }
  }
  return { reached, met };
}

/** `umx entity show` over the covered graphs, from one node (canvas key): its members (the asked one first, then by
 * graph in open order and id), the proposed partners, and the denials that split it. Null when the node is not in a
 * covered graph. */
export function entityOf(world: World, start: string, graphIds: ReadonlyMap<string, string>): Walked | null {
  const gid = graphIds.get(aliasOfKey(start));
  const first: At = `${gid}/${unq(start).id}`;
  if (!world.nodes.has(first) || world.covered.get(gid!) !== aliasOfKey(start)) return null;
  const { reached, met } = walk(world, first);
  const covered = [...world.covered.keys()];
  const grouped = groupOf(world, reached, met);  const r = grouped.root.get(first)!;
  const group = grouped.members.get(r)!;
  const order = new Map(covered.map((g, i) => [g, i]));
  const members: Member[] = [];
  for (const at of reached) {
    const n = world.nodes.get(at);
    if (!n || !group.has(at)) continue;
    members.push({ at, key: n.key, graph: aliasOfKey(n.key), kind: n.kind, matched: at === first ? [{ by: "asked" }] : [] });
  }
  for (const m of members) {
    const mine = world.nodes.get(m.at)!;
    for (const k of mine.keys) {
      if (!usable(world, m.at, mine, k)) continue;
      const shared = members.some((o) => o !== m && o.kind === m.kind && world.nodes.get(o.at)!.keys.includes(k) && usable(world, o.at, world.nodes.get(o.at)!, k));
      if (shared) m.matched.push({ by: "key", key: k });
    }
  }
  const proposed: Proposed[] = [];
  const denied: Denied[] = [];
  // A proposal into a group the entity is denied with is dropped, even one the walk never reached (review R5): its
  // group is found under the same coverage, only for this check.
  const blockedHere = (other: At) => {
    if (grouped.root.has(other)) return blocked(grouped, r, other);
    const partners = grouped.denied.get(r);
    if (!partners || !world.nodes.has(other)) return false;
    if (partners.has(other)) return true;
    const theirs = walk(world, other);
    const g = groupOf(world, theirs.reached, theirs.met);
    const group = g.members.get(g.root.get(other)!)!;
    for (const p of partners) if (group.has(p)) return true;
    return false;
  };
  for (const l of met) {
    const ends = [l.from, l.to];
    ends.forEach((end, i) => {
      const member = members.find((m) => m.at === end);
      if (!member) return;
      const other = ends[1 - i];
      if (l.kind === SAME_AS) {
        // Only a link the group kept (E2c: a skipped one joined nothing).
        if (group.has(other) && !member.matched.some((x) => x.by === "link" && x.edge === l.edge)) member.matched.push({ by: "link", graph: l.owner, edge: l.edge });
        return;
      }
      const n = world.nodes.get(other);
      if (l.kind === NOT_SAME_AS) {
        if (n && grouped.pairs.get(end)?.has(other) && !denied.some((d) => d.at === other)) {
          denied.push({ at: other, key: n.key, graph: aliasOfKey(n.key), with: member.key, edge: l.edge, graph_of_edge: l.owner });
        }
        return;
      }
      if (group.has(other) || blockedHere(other)) return;
      if (n && !proposed.some((p) => p.at === other)) {
        proposed.push({ at: other, key: n.key, graph: aliasOfKey(n.key), with: member.key, edge: l.edge, graph_of_edge: l.owner });
      }
    });
  }
  const rank = (at: At) => [order.get(gidOf(at)) ?? Infinity, idOf(at)] as const;
  const byPlace = (a: At, b: At) => {
    const [ra, ia] = rank(a);
    const [rb, ib] = rank(b);
    return ra - rb || cmp(ia, ib);
  };
  members.sort((a, b) => (a.at === first ? -1 : b.at === first ? 1 : byPlace(a.at, b.at)));
  proposed.sort((a, b) => byPlace(a.at, b.at));
  denied.sort((a, b) => byPlace(a.at, b.at));
  return { members, proposed, denied };
}

/** One entity on the canvas: its members' canvas keys, the number of graphs they are in, and the graphs that hold two
 * or more of them (D69 (3); umx warns entity_graph_twice). */
export type Entity = { members: string[]; graphs: number; twice: string[] };

/** Every entity of two or more covered members, and the proposed pairs between nodes of different entities. */
export type Entities = { list: Entity[]; of: Map<string, number>; proposed: [string, string][] };

export const NO_ENTITIES: Entities = { list: [], of: new Map(), proposed: [] };

/** The entities as groups (for the canvas): umx's joins over every covered node and every link end at once. The fixed
 * join order restricted to one connected part is the order umx uses for it, so each member's own read (entityOf)
 * finds the same group. */
export function entitiesOf(world: World): Entities {
  const reached = new Set<At>(world.nodes.keys());
  const met: Link[] = [];
  const seen = new Set<string>();
  for (const list of world.links.values()) {
    for (const l of list) {
      if (seen.has(l.edge)) continue;
      seen.add(l.edge);
      met.push(l);
      reached.add(l.from);
      reached.add(l.to);
    }
  }
  const grouped = groupOf(world, reached, met);
  const list: Entity[] = [];
  const of = new Map<string, number>();
  for (const ats of grouped.members.values()) {
    const shown = [...ats].filter((a) => world.nodes.has(a));
    if (shown.length < 2) continue;
    const gids = shown.map(gidOf);
    const graphs = new Set(gids).size;
    const members = shown.map((a) => world.nodes.get(a)!.key);
    for (const m of members) of.set(m, list.length);
    const per = new Map<string, number>();
    for (const m of members) per.set(aliasOfKey(m), (per.get(aliasOfKey(m)) ?? 0) + 1);
    list.push({ members, graphs, twice: [...per].filter(([, n]) => n > 1).map(([g]) => g) });
  }
  const proposed: [string, string][] = [];
  for (const l of met) {
    if (l.kind !== POSSIBLY_SAME_AS) continue;
    const [a, b] = [world.nodes.get(l.from), world.nodes.get(l.to)];
    if (!a || !b || a.key === b.key) continue;
    const [ra, rb] = [grouped.root.get(l.from)!, grouped.root.get(l.to)!];
    if (ra === rb || blocked(grouped, ra, l.to)) continue;
    proposed.push([a.key, b.key]);
  }
  return { list, of, proposed };
}


/** Whether the canvas leaves an identity edge out: a not_same_as always, as a denial is no connection and the panel
 * lists it (ui-expert N1); a same_as or possibly_same_as a thread already draws (ui-expert N4).
 * `proposedPairs` holds "<a>|<b>" both ways for each proposed pair. */
export function identityEdgeHidden(kind: string, s: string, t: string, entities: Entities, proposedPairs: ReadonlySet<string>): boolean {
  if (kind === NOT_SAME_AS) return true;
  if (kind === SAME_AS) return entities.of.has(s) && entities.of.get(s) === entities.of.get(t);
  if (kind === POSSIBLY_SAME_AS) return proposedPairs.has(`${s}|${t}`);
  return false;
}

/** Whether the edge reducer leaves an edge out whatever its ends: its relation turned off, an edge inside one merged
 * entity (`together`), or an identity edge identityEdgeHidden leaves out. The reducer and the hub count (E1-N1) both
 * ask this one predicate, so a head is never shrunk by an edge that is not drawn (reviewer B1). */
export type LeftOut = { off: ReadonlySet<string>; together: (s: string, t: string) => boolean; entities: Entities; proposedPairs: ReadonlySet<string> };
export function edgeLeftOut(graph: ViewGraph, rules: LeftOut): (edge: string) => boolean {
  return (edge) => {
    const [s, t] = graph.extremities(edge);
    const kind = graph.getEdgeAttribute(edge, "kind");
    return rules.off.has(kind) || rules.together(s, t) || identityEdgeHidden(kind, s, t, rules.entities, rules.proposedPairs);
  };
}

/** The member pairs a thread joins: every pair up to SMALL members, else each member to the first. */
const SMALL = 6;
export function threadPairs(members: readonly string[]): [string, string][] {
  const out: [string, string][] = [];
  if (members.length <= SMALL) {
    for (let i = 0; i < members.length; i++) for (let j = i + 1; j < members.length; j++) out.push([members[i], members[j]]);
  } else for (let i = 1; i < members.length; i++) out.push([members[0], members[i]]);
  return out;
}

/** A member's record as `umx node get` gives it, with its graph. */
export type MemberRecord = { graph: string; key: string; node: Record<string, unknown> };
export type MergedValue = { value: unknown; graph: string };
export type MergedField = { name: string; value: unknown; graph: string; others: MergedValue[] };
export type Merged = { title: string; from: string; updated: string; fields: MergedField[] };

/** Fields the merged panel never lists as data: the record's own and the keys (shown by the Entity section). */
const NOT_FIELDS = new Set([
  "id", "type", "title", "status", "note", "evidence", "evidence_kinds", "status_history", "created", "updated",
  "created_by", "address", "graph", "entity_key", "authored_by_session", "edited_by_session",
]);

const updatedOf = (r: MemberRecord) => (typeof r.node.updated === "string" ? r.node.updated : "");
const present = (v: unknown) => v !== undefined && v !== null && v !== "";

/** D69 (2), merged mode: the title and each field from the most recently updated member that has it (ties to the
 * earlier member); every other member's different value is kept, with its graph, for "+n other". */
export function mergeRecords(records: readonly MemberRecord[]): Merged | null {
  if (records.length === 0) return null;
  const order = records.map((r, i) => ({ r, i })).sort((a, b) => (updatedOf(b.r) > updatedOf(a.r) ? 1 : updatedOf(b.r) < updatedOf(a.r) ? -1 : a.i - b.i));
  const latest = order[0].r;
  const names: string[] = [];
  for (const { r } of order) for (const k of Object.keys(r.node)) if (!NOT_FIELDS.has(k) && !names.includes(k)) names.push(k);
  const fields: MergedField[] = [];
  for (const name of names.sort()) {
    const having = order.filter(({ r }) => present(r.node[name]));
    if (having.length === 0) continue;
    const chosen = having[0].r;
    const same = JSON.stringify(chosen.node[name]);
    // A hidden value (umx flagged it) is never the same as another: two hidden values may differ (ws009 M2b).
    const differs = (v: unknown) => v === HIDDEN_VALUE || chosen.node[name] === HIDDEN_VALUE || JSON.stringify(v) !== same;
    const others = having.slice(1).filter(({ r }) => differs(r.node[name])).map(({ r }) => ({ value: r.node[name], graph: r.graph }));
    fields.push({ name, value: chosen.node[name], graph: chosen.graph, others });
  }
  return { title: String(latest.node.title ?? ""), from: latest.graph, updated: updatedOf(latest), fields };
}

/** Where a member is drawn while entities are merged: at its entity's centroid (of its shown members), the size of
 * its biggest member and a bit more. `head` is the member whose title the merged node shows (the latest updated,
 * D69 (2)); `title` is that title. Hidden members stay hidden and out of the centroid. */
export type Place = { x: number; y: number; size: number; head: boolean; entity: number; title: string; graphs: string[] };

export const MERGED_GROWTH = 1.25;

/** Where a node is laid out, merged or not. */
export const homeOf = (a: { x: number; y: number; homeX?: number; homeY?: number }) => ({ x: a.homeX ?? a.x, y: a.homeY ?? a.y });

/** `shown`, when given, is this render's visibility (the filters as they are now); else the node's hidden attribute,
 * which the canvas sets only after a filter changes (ws009 M1c review N1). */
export function mergedPlaces(graph: ViewGraph, entities: Entities, visible?: (key: string) => boolean): Map<string, Place> {
  const out = new Map<string, Place>();
  const isShown = visible ?? ((m: string) => !graph.getNodeAttribute(m, "hidden"));
  entities.list.forEach((e, i) => {
    const shown = e.members.filter((m) => graph.hasNode(m) && isShown(m));
    if (shown.length < 2) return;
    let [x, y, size] = [0, 0, 0];
    let head = shown[0];
    for (const m of shown) {
      const a = graph.getNodeAttributes(m);
      const home = homeOf(a);
      x += home.x;
      y += home.y;
      size = Math.max(size, a.size);
      if ((a.updated ?? "") > (graph.getNodeAttribute(head, "updated") ?? "")) head = m;
    }
    const title = graph.getNodeAttribute(head, "label");
    const graphs = [...new Set(shown.map((m) => graph.getNodeAttribute(m, "graph")))];
    for (const m of shown) out.set(m, { x: x / shown.length, y: y / shown.length, size: size * MERGED_GROWTH, head: m === head, entity: i, title, graphs });
  });
  return out;
}

/** Merges `entities` on the canvas, or (with none) puts every member back where it is laid out: each shown member's
 * x and y become its entity's centroid, its own kept as homeX and homeY. Only writes when something moves. */
export function applyMerge(graph: ViewGraph, entities: Entities) {
  let merged = false;
  graph.forEachNode((_n, a) => {
    if (a.homeX !== undefined) merged = true;
  });
  if (merged) {
    graph.updateEachNodeAttributes((_n, a) => {
      if (a.homeX === undefined) return a;
      a.x = a.homeX;
      a.y = a.homeY ?? a.y;
      delete a.homeX;
      delete a.homeY;
      return a;
    });
  }
  if (entities.list.length === 0) return;
  const places = mergedPlaces(graph, entities);
  if (places.size === 0) return;
  graph.updateEachNodeAttributes((n, a) => {
    const p = places.get(n);
    if (!p) return a;
    a.homeX = a.x;
    a.homeY = a.y;
    a.x = p.x;
    a.y = p.y;
    return a;
  });
}

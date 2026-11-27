// The links of every member of a merged entity (PRD 005 centre-14): with Merge entities on, the panel lists the
// neighbours of all the members of the entity the person selected, each member read from umx as one node is.
import type { neighbourLinks } from "@/lib/graph/build";
import type { Neighbour, NodeDetail } from "@/lib/types";

type Links = ReturnType<typeof neighbourLinks>;
export type MemberDetail = { key: string; detail: NodeDetail | null };
export type MemberLinks = { neighbours: Neighbour[]; truncated: boolean; failed: number; links: Links };

/** Member reads at once. */
export const MEMBERS_AT_ONCE = 6;

/** Each member's node read, a few at once; a member umx would not read is null (the panel counts it). */
export async function readMembers(keys: readonly string[], read: (key: string) => Promise<NodeDetail>, atOnce = MEMBERS_AT_ONCE): Promise<MemberDetail[]> {
  const out: MemberDetail[] = [];
  for (let i = 0; i < keys.length; i += atOnce) {
    const batch = keys.slice(i, i + atOnce);
    const got = await Promise.allSettled(batch.map(read));
    got.forEach((g, j) => out.push({ key: batch[j], detail: g.status === "fulfilled" ? g.value : null }));
  }
  return out;
}

/**
 * Every member's neighbours, in member order (the selected member first), each link once. A link to another member of
 * the same entity is left out: it has no length on the merged node, and the Entity section lists it. `linksOf` maps a
 * member's rows to the canvas from that member's own graph, so each row keeps its own target, origin and graph name.
 */
export function unionNeighbours(parts: readonly MemberDetail[], members: ReadonlySet<string>, linksOf: (member: string) => Links): MemberLinks {
  const of = new Map<Neighbour, Links>();
  const seen = new Set<string>();
  const neighbours: Neighbour[] = [];
  let [truncated, failed] = [false, 0];
  for (const p of parts) {
    if (!p.detail) {
      failed++;
      continue;
    }
    truncated ||= p.detail.neighboursTruncated;
    const links = linksOf(p.key);
    for (const n of p.detail.neighbours) {
      const far = links.target(n);
      if (far !== null && members.has(far)) continue;
      const key = JSON.stringify([n.way, n.relation, n.edge, far ?? `${n.graph}/${n.id}`]);
      if (seen.has(key)) continue;
      seen.add(key);
      neighbours.push(n);
      of.set(n, links);
    }
  }
  const first = linksOf(parts[0]?.key ?? "");
  const pick = (n: Neighbour) => of.get(n) ?? first;
  return {
    neighbours, truncated, failed,
    links: { target: (n) => pick(n).target(n), edgeOrigin: (n) => pick(n).edgeOrigin(n), graphName: (n) => pick(n).graphName(n) },
  };
}

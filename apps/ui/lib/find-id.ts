// Find by id (the Find box): a full node id, a unique prefix of one, or a umx: address (`umx:<alias>/<id>`), matched
// against the nodes on the canvas, as umx resolves ids (case-insensitive Crockford base32). Pure: the view jumps.
import { qid, unq } from "@/lib/graph/keys";

/** What the box's text names, as an id: one node's canvas key, several, or nothing (null: not id-like). */
export type IdMatch = { kind: "one"; key: string } | { kind: "many"; count: number } | { kind: "none" };

/** A node id or a prefix of one: 4 to 26 Crockford base32 letters and digits, at least one digit (so a word is not). */
const ID_PART = /^(?=.*\d)[0-9A-HJKMNP-TV-Z]{4,26}$/i;
const ADDRESS = /^umx:([a-z][a-z0-9_-]{0,63})\/([^/\s]+)$/;

/** The id or prefix and graph `text` names, or null when it does not look like one. */
export function idQuery(text: string): { graph: string | null; id: string } | null {
  const t = text.trim();
  const address = ADDRESS.exec(t);
  const [graph, id] = address ? [address[1], address[2]] : [null, t];
  return ID_PART.test(id) ? { graph, id: id.toUpperCase() } : null;
}

/** The canvas nodes `text` names by id: in its address's graph only, else in every open graph. */
export function findById(text: string, keys: Iterable<string>): IdMatch | null {
  const q = idQuery(text);
  if (!q) return null;
  if (q.graph !== null && q.id.length === 26) {
    for (const key of keys) if (key === qid(q.graph, q.id)) return { kind: "one", key };
    return { kind: "none" };
  }
  const exact: string[] = [];
  const found: string[] = [];
  for (const key of keys) {
    const { alias, id } = unq(key);
    if ((q.graph === null || alias === q.graph) && id.startsWith(q.id)) (id === q.id ? exact : found).push(key);
  }
  // An exact id wins over longer ids it prefixes; the same id in two open graphs names both.
  const hits = exact.length > 0 ? exact : found;
  return hits.length === 1 ? { kind: "one", key: hits[0] } : hits.length > 1 ? { kind: "many", count: hits.length } : { kind: "none" };
}

// umx's read-side secret flags (ws002 F188, usage 174) as the view applies them (ws009 M2b, plan 176; security's
// rulings on M2 and M2b). `node get`, `node list` and `entity show` name each shown value umx's own secret check flags
// (`field:<name>`, `key:<i>`, `evidence:<i>`, `status_reason:<i>`, `status_evidence:<i>` (the i-th status move's
// evidence pointer, ws008 TM6; plan 192), `evidence_removed:<i>`; list positions counted from 1, F217). The server
// replaces each flagged value before the reply leaves it, from the flags of that same reply, so a flagged value never
// reaches the browser: a title becomes "<type> · <short id>", a key its plain scheme and "(hidden key)", an evidence
// pointer a new { source: hidden, kind } with every other key dropped, anything else "(hidden: may be a secret)". The
// flags themselves are dropped; they are never shown as values.
import { HIDDEN_KEY, HIDDEN_VALUE, hiddenKeyText } from "@/lib/graph/entities";

export { HIDDEN_VALUE };

export const SECRET_SUSPECTED = "secret_suspected";
/** The panel's hint beside a title it hides (security on M2b). */
export const TITLE_HINT = "hidden: looks like a secret";

/** The last six characters of a node id: ULIDs share their leading (time) characters, not their last ones. */
export const shortId = (id: string) => id.slice(-6);
/** A flagged title, wherever the view would show one (security on M2b). */
export const hiddenTitle = (type: string, id: string) => `${type} · ${shortId(id)}`;

/** A reply object's flags: strings only. */
export function flagsOf(value: unknown): string[] {
  const flags = (value as Record<string, unknown> | null)?.[SECRET_SUSPECTED];
  return Array.isArray(flags) ? flags.filter((f): f is string => typeof f === "string") : [];
}

/** Whether a reply object's title is flagged. */
export const titleFlagged = (value: unknown) => flagsOf(value).includes("field:title");

/** Fields that hold no graph text (ids, enums, stamps): left as they are when a flag umx adds later is not understood. */
const PLAIN = new Set(["id", "type", "status", "created", "updated", "created_by", "address", "graph", "evidence_kinds"]);

/** The array index a list flag names: its position counted from 1 (F217; an older umx's 0-based flags are moved up
 * when the envelope is read, see `flagsFromOne` in lib/umx). Position 0 is no entry: the flag is not understood. */
const index = (flag: string, prefix: string): number | null => {
  if (!flag.startsWith(prefix)) return null;
  const n = Number(flag.slice(prefix.length));
  return Number.isSafeInteger(n) && n >= 1 ? n - 1 : null;
};

/** A flagged evidence pointer (node evidence or a status move's): a new object with the source hidden and only a
 * string kind kept; selector, hash, by_session and every other key are dropped (security on plan 192). */
const hiddenPointer = (item: unknown): Record<string, unknown> => {
  const kind = (item as Record<string, unknown> | null)?.kind;
  return { source: HIDDEN_VALUE, ...(typeof kind === "string" ? { kind } : {}) };
};

/** A node object (`node get`, a `node list` row, an entity member) with every value its own flags name replaced, and
 * its flags dropped. `titleHidden` says the title was replaced. A flag the view does not know hides every field that
 * can hold graph text, so a new kind of flag fails closed. */
export function redactNode(node: Record<string, unknown>, flags: readonly string[] = flagsOf(node)): { node: Record<string, unknown>; titleHidden: boolean } {
  const out: Record<string, unknown> = { ...node };
  delete out[SECRET_SUSPECTED];
  let titleHidden = false;
  const list = (name: string) => {
    const v = out[name];
    if (!Array.isArray(v)) return null;
    if (v === node[name]) out[name] = [...v];
    return out[name] as unknown[];
  };
  for (const flag of flags) {
    let i: number | null;
    if (flag === "field:title") {
      out.title = hiddenTitle(String(node.type ?? ""), String(node.id ?? ""));
      titleHidden = true;
    } else if (flag === "field:entity_key") {
      out.entity_key = HIDDEN_KEY;
    } else if (flag === "field:locator_source" || flag === "field:locator") {
      if ("locator" in out) out.locator = HIDDEN_VALUE;
      if ("locator_source" in out) out.locator_source = HIDDEN_VALUE;
    } else if (flag.startsWith("field:")) {
      const name = flag.slice("field:".length);
      if (name in out) out[name] = HIDDEN_VALUE;
    } else if ((i = index(flag, "key:")) !== null) {
      const keys = list("entity_key");
      if (keys && i < keys.length) keys[i] = typeof keys[i] === "string" ? hiddenKeyText(keys[i] as string) : HIDDEN_KEY;
    } else if ((i = index(flag, "evidence:")) !== null) {
      const items = list("evidence");
      if (items && i < items.length) items[i] = hiddenPointer(items[i]);
    } else if ((i = index(flag, "status_reason:")) !== null) {
      const items = list("status_history");
      if (items && i < items.length && typeof items[i] === "object" && items[i] !== null) items[i] = { ...(items[i] as object), reason: HIDDEN_VALUE };
    } else if ((i = index(flag, "status_evidence:")) !== null) {
      // A move's own evidence pointer (ws008 TM6): replaced as node evidence is, the rest of the move kept.
      const items = list("status_history");
      if (items && i < items.length && typeof items[i] === "object" && items[i] !== null && "evidence" in (items[i] as object)) {
        const move = items[i] as Record<string, unknown>;
        items[i] = { ...move, evidence: hiddenPointer(move.evidence) };
      }
    } else if ((i = index(flag, "evidence_removed:")) !== null) {
      const items = list("evidence_removed");
      if (items && i < items.length && typeof items[i] === "object" && items[i] !== null) {
        const entry = { ...(items[i] as Record<string, unknown>) };
        for (const k of ["reason", "removed"]) if (k in entry) entry[k] = HIDDEN_VALUE;
        items[i] = entry;
      }
    } else {
      for (const name of Object.keys(out)) {
        if (PLAIN.has(name)) continue;
        if (name === "title") {
          out.title = hiddenTitle(String(node.type ?? ""), String(node.id ?? ""));
          titleHidden = true;
        } else out[name] = HIDDEN_VALUE;
      }
    }
  }
  return { node: out, titleHidden };
}

/** A reply object's title and keys as the view may show them, through redactNode with all of its flags (review
 * N1): a flag the view does not know hides both, as it hides a whole node. Keys that are not a list are none. */
export function projected(row: Record<string, unknown>, id: string = typeof row.id === "string" ? row.id : ""): { title: string; keys: string[]; titleHidden: boolean } {
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const { node, titleHidden } = redactNode({ id, type: str(row.type), title: str(row.title), ...("entity_key" in row ? { entity_key: row.entity_key } : {}) }, flagsOf(row));
  const keys = Array.isArray(node.entity_key) ? node.entity_key.filter((k): k is string => typeof k === "string") : [];
  return { title: str(node.title), keys, titleHidden };
}

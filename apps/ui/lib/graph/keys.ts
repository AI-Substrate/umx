// Graph-qualified keys (ws009 M1a, plan 133): several graphs share one canvas, so every node and edge on it is keyed
// by its graph's alias and its own id, `<alias>/<id>`, the same shape as umx's address `umx:<alias>/<id>`. Equal ids
// in two graphs never collide. An alias never holds "/" (lib/umx ALIAS), so the first "/" splits a key.

/** The most graphs one view opens at once: a poll, a row read and a launch link name at most this many. */
export const MAX_OPEN = 12;

export function qid(alias: string, id: string): string {
  return `${alias}/${id}`;
}

/** The alias and id in a key; a key with no "/" has an empty alias. */
export function unq(key: string): { alias: string; id: string } {
  const i = key.indexOf("/");
  return i < 0 ? { alias: "", id: key } : { alias: key.slice(0, i), id: key.slice(i + 1) };
}

export function aliasOfKey(key: string): string {
  return unq(key).alias;
}

/** The open graphs as one key for state that belongs to exactly this set (selection, agent feed, canvas). */
export function setKey(aliases: readonly string[]): string {
  return aliases.join(",");
}

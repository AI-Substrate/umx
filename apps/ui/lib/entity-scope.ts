// What the node panel and the agent strip may use of an entity read or an agent's offer (ws009 M2; review B3):
// only what was made for the node and the open set on screen now, so nothing from a closed graph outlives it.
import type { Entities } from "@/lib/graph/entities";

/** `<node>|<open graphs>|<merged>`: what an entity reply was asked for, without the live tick. */
export function entityScope(node: string | null, aliases: readonly string[] | null, merged: boolean): string | null {
  return node && aliases ? `${node}|${aliases.join(",")}|${merged ? 1 : 0}` : null;
}

/** The scope part of an ask (`<scope>|<live tick>`). */
export const scopeOfAsk = (ask: string) => ask.slice(0, ask.lastIndexOf("|"));

/** A stored reply, only when it was asked for this scope. */
export function replyFor<T extends { ask: string }>(reply: T | null, scope: string | null): T | null {
  return reply && scope !== null && scopeOfAsk(reply.ask) === scope ? reply : null;
}

/** An offer's members now: only on the view it was made on, from the entities on screen, and only those on the canvas. */
export function offerMembers(offer: { node: string; view: string } | null, viewKey: string | null, entities: Entities, onCanvas: (key: string) => boolean): string[] | null {
  if (!offer || offer.view !== viewKey) return null;
  const at = entities.of.get(offer.node);
  return at === undefined ? null : entities.list[at].members.filter(onCanvas);
}

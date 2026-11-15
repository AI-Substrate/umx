// The node panel's similar_to state. A read belongs to one graph and one node, and to the request that asked for it:
// a reply for another graph, node or an older request is dropped (review R199-3).
import type { ApiError, SearchResult } from "@/lib/types";

export type SimilarState = {
  alias: string;
  id: string;
  /** Which request this is; only its reply lands. */
  req: number;
  loading: boolean;
  result: SearchResult | null;
  error: ApiError | null;
} | null;

export function similarAsked(alias: string, id: string, req: number): SimilarState {
  return { alias, id, req, loading: true, result: null, error: null };
}

/** The reply to request `req`, or the state unchanged when the state has moved on. */
export function similarReplied(state: SimilarState, req: number, reply: { result: SearchResult } | { error: ApiError }): SimilarState {
  if (!state || state.req !== req) return state;
  return "result" in reply ? { ...state, loading: false, result: reply.result } : { ...state, loading: false, error: reply.error };
}

/** What the panel shows for the graph and node on screen: nothing unless the state is for exactly those. */
export function similarFor(state: SimilarState, alias: string | null, id: string | null): SimilarState {
  return state && state.alias === alias && state.id === id ? state : null;
}

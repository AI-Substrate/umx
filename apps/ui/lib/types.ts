import type { LiveCursor } from "@/lib/live";

/** One row of `umx graph list --all` (this machine's graph index, req-116). */
export type GraphSummary = {
  alias: string;
  id: string;
  /** null when umx does not show them (a graph that is not loaded and not open). */
  nodes: number | null;
  edges: number | null;
  /** Any named-visibility policy (secured, named+any, named+trusted): opened only when named or clicked. */
  secured: boolean;
  /** loaded, idle, not_reloaded or unloaded. */
  state: string;
  /** present, missing or outside_roots. */
  folder: string;
  /** loaded, or idle in a present folder: a named read loads an idle graph on demand (F72). */
  openable: boolean;
  /** Why it cannot be opened, and the command that fixes it; null when it can. */
  reason: string | null;
  fix: string | null;
};

/** A saved scope (`umx scope list`) as the picker offers it (plan 133). Members are resolved through the graph index:
 * `graphs` are the ones it opens (openable, not secured). Secured members are counted, never shown by name (d-20);
 * `securedGraphs` (openable ones) open only while the person's "include secured graphs" toggle is on (D69 (7)). */
export type ScopeSummary = { name: string; graphs: string[]; securedGraphs: string[]; secured: number; unavailable: number };

/** [id, type, title, status, updated, entity_key]: the last two only where entities need them (ws009 M2): a node with
 * keys, or an end of an identity edge, and every node a live change reads. */
export type NodeRow = [id: string, type: string, title: string, status: string, updated?: string, keys?: string[]];
/** [id, type, from, to] */
export type EdgeRow = [string, string, string, string];
/** An edge with an end in another graph: [id, type, from graph id, from, to graph id, to, unresolved]. Drawn only when
 * both graphs are open on the canvas (plan 133). `unresolved`: umx gave an end as graph_not_loaded to this graph's
 * own read, as it does for a secured graph nobody named; it is drawn only when the person opened that graph too. */
export type ExternalRow = [string, string, string, string, string, string, boolean];

export type Snapshot = {
  graph: string;
  /** The graph's stable id, from `umx graph list`. */
  graphId: string;
  nodes: NodeRow[];
  /** Only edges with both ends resolved in this graph. */
  edges: EdgeRow[];
  /** Edges stored in this graph with an end in another graph, also counted in omittedEdges (plan 133). */
  external?: ExternalRow[];
  /** Ids of edges stored here with an end umx could not resolve (edge_broken, copy_ambiguous...): never drawn. */
  broken?: string[];
  /** The graph's symmetric relations (`umx ontology show`, built-in ones included): drawn as lines, not arrows. */
  symmetric?: string[];
  totalNodes: number;
  totalEdges: number;
  truncated: boolean;
  /** Edges left out of the canvas, by reason: an end in another graph, or an end umx could not resolve. */
  omittedEdges: { otherGraph: number; unresolved: number };
  /** umx's warnings from the reads, as given: graph_loaded_on_demand when an idle graph was loaded to open it. */
  warnings?: ApiError[];
  /** umx's hints from the reads, as given (D69 (9)). */
  hints?: ApiError[];
  /** The graph's revision before it was read (the change feed, plan 127); null when the feed did not know it. */
  live?: LiveCursor | null;
};

type NeighbourBase = {
  way: string;
  relation: string;
  edge: string;
  id: string;
  /** The neighbour's graph id. */
  graph: string;
  /** Its alias when that graph is loaded, else null. */
  graphAlias: string | null;
  /** True only for a resolved node in the graph on screen: the only kind the view moves to. */
  local: boolean;
};

/** A neighbour umx resolved to a node. */
export type ResolvedNeighbour = NeighbourBase & { kind: "node"; type: string; title: string; status: string };

/** A neighbour umx could not resolve (graph_not_loaded, copy_ambiguous, edge_broken...): no type, title or status. */
export type StateNeighbour = NeighbourBase & { kind: "state"; state: string; candidates: string[] };

export type Neighbour = ResolvedNeighbour | StateNeighbour;

export type NodeDetail = {
  /** The node as `umx node get` gives it, each value umx flags replaced (ws009 M2b, lib/secret-flags). */
  node: Record<string, unknown>;
  /** Its title was flagged, so `node.title` is "<type> · <short id>"; the panel says so. */
  titleHidden?: boolean;
  neighbours: Neighbour[];
  neighboursTruncated: boolean;
};

export type ApiError = { code: string; message: string; next?: string | null };

export type SearchMode = "title" | "hybrid";

/** One search, find or similar_to hit: the node and how it was found. */
export type Hit = {
  id: string;
  type: string;
  title: string;
  status: string;
  /** node find's score, search's fused score, or the similar_to cosine. */
  score: number | null;
  /** search only: word, vector, both or exact. */
  legs: string | null;
  /** search only: the field that equals the query. */
  exact: string | null;
  /** The hit's graph, set when several graphs are open (plan 133). */
  graph?: string;
};

export type SearchResult = {
  mode: SearchMode | "similar";
  /** The text umx was given (trimmed, spaces collapsed); empty for similar. */
  query: string;
  hits: Hit[];
  /** umx's warnings, as given (embedder_mismatch, similar_to_incomplete...). */
  warnings: ApiError[];
  /** search's word_leg note, or the similar_to lists' model, k and floor. */
  note: string | null;
  /** umx's hints, as given (D69 (9)): similarity_unavailable, similarity_incomplete, search_results_cut... */
  hints?: ApiError[];
  /** Rows left out because they are not nodes of the graph on screen. */
  foreign: number;
  /** Open graphs that found nothing while another did, as one quiet line (ws009 M2). */
  quiet?: string;
};

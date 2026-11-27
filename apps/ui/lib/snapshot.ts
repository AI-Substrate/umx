import { PAGE, type createUmx, type Renamed } from "@/lib/umx";
import { alias as checkAlias, ALIAS, hintsOf, UmxError, warningsOf } from "@/lib/umx";
import { MAX_OPEN } from "@/lib/graph/keys";
import { shadowCache } from "@/lib/shadows-read";
import { keysOf } from "@/lib/graph/entities";
import { shownTitle, vouchTitles } from "@/lib/flagged-read";
import { hiddenTitle, redactNode } from "@/lib/secret-flags";
import type { ApiError, EdgeRow, ExternalRow, GraphSummary, HubDeclaration, Neighbour, NodeDetail, NodeRow, ResolvedNeighbour, ScopeSummary, Snapshot } from "@/lib/types";

type Reader = ReturnType<typeof createUmx>;
type Rows<T> = { rows: T[]; total: number; truncated: boolean };

/** The most nodes and edges the view loads; past these it opens truncated, with a banner. */
export const NODE_CAP = 100_000;
export const EDGE_CAP = 200_000;

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const count = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** Every policy with named visibility (secured is named + local) is opened only when named or clicked (D53). An
 * unknown policy counts as named: the view never shows more than umx would. */
export function isNamed(policy: string | null): boolean {
  return policy !== "open" && policy !== "open+trusted" && policy !== "open+local";
}

/** Why a remembered graph cannot be opened, and the command that fixes it, or null when it can be. */
export function notOpenable(row: { alias: string; state: string; folder: string; reload: string | null }): { reason: string; fix: string } | null {
  const reload = row.reload?.startsWith("umx ") ? row.reload : `umx graph load <folder> --as ${row.alias}`;
  if (!ALIAS.test(row.alias)) return { reason: "Its alias is not one this view can open.", fix: "umx graph list --all" };
  if (row.folder === "missing") {
    return { reason: "Its folder holds no graph now (moved, renamed or deleted).", fix: `umx graph load <new folder> --as ${row.alias}` };
  }
  if (row.folder !== "present") {
    return { reason: "Its folder is outside the graph roots now, so umx does not look at it.", fix: "umx config set roots <folders>" };
  }
  if (row.state === "loaded" || row.state === "idle") return null;
  if (row.state === "unloaded") return { reason: "It was unloaded; the view never loads a graph by itself.", fix: reload };
  if (row.state === "not_reloaded") return { reason: "It did not come back when the daemon started.", fix: reload };
  return { reason: "umx lists it in a state this view does not know.", fix: "umx graph list --all" };
}

/** `umx graph list --all`: the loaded graphs, then every graph this machine remembers (req-116). Paths stay on the
 * server, except inside the command that reloads a graph. */
export async function listGraphs(read: Reader): Promise<GraphSummary[]> {
  const env = await read<Rows<Record<string, unknown>>>("graph list");
  const reloads: (string | null)[] = [];
  const rows = env.data.rows.map((row) => {
    const g = {
      alias: String(row.alias),
      state: str(row.state) ?? "",
      folder: str(row.folder) ?? "",
      reload: str(row.reload),
    };
    const why = notOpenable(g);
    // A named graph's totals never reach the browser, though umx gives them for a loaded one (review R222-1).
    const secured = isNamed(str(row.policy));
    const summary: GraphSummary = {
      alias: g.alias,
      id: String(row.id),
      nodes: secured ? null : count(row.nodes),
      edges: secured ? null : count(row.edges),
      secured,
      state: g.state,
      folder: g.folder,
      openable: why === null,
      reason: why?.reason ?? null,
      fix: why?.fix ?? null,
    };
    reloads.push(g.reload);
    return summary;
  });
  return sharedAliases(rows, reloads);
}

/**
 * Rows that share an alias (umx remembers a graph per folder, so two folders can carry one alias): the view reads by
 * alias, and a named read reaches the loaded one, so only that row opens. With none loaded, none opens: umx would not
 * know which folder is meant (as graphIds refuses it). The others say why, and the command that loads them under a
 * new alias.
 */
export function sharedAliases(rows: GraphSummary[], reloads: readonly (string | null)[]): GraphSummary[] {
  const byAlias = new Map<string, GraphSummary[]>();
  for (const r of rows) byAlias.set(r.alias, [...(byAlias.get(r.alias) ?? []), r]);
  return rows.map((r, i) => {
    const same = byAlias.get(r.alias)!;
    if (same.length < 2 || !r.openable) return r;
    const loaded = same.some((o) => o.state === "loaded");
    if (loaded && r.state === "loaded") return r;
    const reload = reloads[i];
    const fix = reload?.startsWith("umx ") ? reload.replace(/--as \S+$/, "--as <new alias>") : "umx graph list --all";
    const reason = loaded
      ? `Another folder's graph is loaded as ${r.alias}; reading ${r.alias} reads that one.`
      : `umx remembers ${r.alias} in ${same.length} folders; load the one you mean under its own alias.`;
    return { ...r, openable: false, reason, fix };
  });
}

const SCOPE_NAME = /^[a-z][a-z0-9_]{0,63}$/;

/**
 * `umx scope list`, each member resolved by graph id through the graph index (plan 133). A scope opens only its members
 * the picker would open and that are not secured: a secured member is counted, never named, and opens only when the
 * person picks it (d-20). Folders and saved alias hints stay on the server.
 */
export async function listScopes(read: Reader): Promise<ScopeSummary[]> {
  const graphs = await listGraphs(read);
  const env = await read<Rows<Record<string, unknown>>>("scope list");
  const out: ScopeSummary[] = [];
  for (const row of env.data.rows ?? []) {
    const name = str(row.name);
    if (name === null || !SCOPE_NAME.test(name)) continue;
    const scope: ScopeSummary = { name, graphs: [], securedGraphs: [], secured: 0, unavailable: 0 };
    for (const m of Array.isArray(row.members) ? (row.members as Record<string, unknown>[]) : []) {
      const rows = graphs.filter((g) => g.id === str(m.id));
      const g = rows.find((r) => r.state === "loaded") ?? rows[0];
      if (m.secured === true || g?.secured) {
        scope.secured++;
        if (g?.openable && !scope.securedGraphs.includes(g.alias)) scope.securedGraphs.push(g.alias);
      } else if (!g || !g.openable) scope.unavailable++;
      else if (!scope.graphs.includes(g.alias)) scope.graphs.push(g.alias);
    }
    out.push(scope);
  }
  return out;
}

/** Pages `verb` 10k rows at a time, one request after another, up to `cap` rows. */
async function pages<T>(read: Reader, verb: "node list" | "edge list", graph: string, cap: number) {
  const rows: T[] = [];
  const warnings: ApiError[] = [];
  const hints: ApiError[] = [];
  let total = 0;
  for (let offset = 0; offset < cap; offset += PAGE) {
    const env = await read<Rows<T>>(verb, { graph, offset });
    warnings.push(...warningsOf(env));
    hints.push(...hintsOf(env));
    total = env.data.total;
    rows.push(...env.data.rows);
    if (env.data.rows.length < PAGE || rows.length >= total) break;
  }
  return { rows: rows.slice(0, cap), total, warnings, hints };
}

/**
 * The graph on screen and the loaded graphs by id; the alias's id is how rows are told local from external. Read from
 * the index before any named read, so the view never makes umx load a graph it may not open: a named read of a
 * `not_reloaded` graph would load it on demand, and one of an unloaded graph is refused anyway.
 *
 * An alias the index does not list may be a former alias (plan 215, `umx graph rename`): the index keeps only live
 * aliases. Then umx is asked where `umx:<alias>` leads (liveAlias), at most once per alias. A graph it leads to that
 * the picker would open refuses the read with `ui_graph_renamed`, naming the live alias and graph id, so the browser
 * switches; umx's own refusal (former_alias_ambiguous, graph_not_loaded) is passed on as it is and switches nothing.
 */
export async function graphIds(read: Reader, alias: string, probes: Probes = sharedProbes()) {
  checkAlias(alias);
  const pick = (graphs: GraphSummary[]) => {
    const rows = graphs.filter((g) => g.alias === alias);
    return { graphs, rows, local: rows.find((g) => g.state === "loaded") ?? (rows.length === 1 ? rows[0] : undefined) };
  };
  // A kept index only ever says yes: anything it would refuse is read again fresh first (a rename, a load, a reload).
  let found = pick(await checkedIndex(read));
  if (!found.local?.openable) found = pick(await freshIndex(read));
  const { graphs, rows, local } = found;
  if (rows.length === 0) {
    const renamed = await liveAlias(read, alias, graphs, probes);
    if (renamed) {
      shadowCache().forget(alias);
      throw new UmxError("ui_graph_renamed", `${alias} was renamed to ${renamed.to}.`, null, 409, renamed);
    }
    throw new UmxError("ui_graph_not_listed", `${alias} is not in umx graph list --all.`, "umx graph list --all", 404);
  }
  if (!local) {
    throw new UmxError("ui_graph_ambiguous", `umx remembers ${alias} in ${rows.length} folders; load the one you mean.`, "umx graph list --all", 409);
  }
  if (!local.openable) throw new UmxError("ui_graph_not_open", local.reason ?? "This graph cannot be opened.", local.fix, 409);
  const loaded = graphs.filter((g) => g.state === "loaded");
  return { localId: local.id, aliasOf: new Map([...loaded, local].map((g) => [g.id, g.alias])) };
}

/** How long one graph index read answers the checks before named reads (graphIds), from when it lands: as long as the
 * change feed trusts a passed check (lib/events CHECK_MS). Opening several graphs, a spine and the change feed each
 * check the index; `umx graph list --all` looks at every remembered graph's folder, so it is the slowest read a
 * snapshot makes (seconds on a busy machine, against tens of ms for the graph's own rows). A kept index only ever
 * says yes: graphIds reads it fresh before refusing. The picker's list (`/api/graphs`) is always read fresh, and
 * then kept for the checks. */
export const INDEX_MS = 10_000;
const indexes = new WeakMap<Reader, { landed: number; graphs: Promise<GraphSummary[]> }>();

/** The graph index for the checks: one read shared by every caller while it runs, and kept INDEX_MS once it lands. A
 * failed read is forgotten, so the next caller asks again. */
export function checkedIndex(read: Reader, now: () => number = Date.now): Promise<GraphSummary[]> {
  const was = indexes.get(read);
  if (was && now() - was.landed < INDEX_MS) return was.graphs;
  return freshIndex(read, now);
}

/** A new index read, which the checks then share (checkedIndex). */
export function freshIndex(read: Reader, now: () => number = Date.now): Promise<GraphSummary[]> {
  const entry = { landed: Infinity, graphs: listGraphs(read) };
  indexes.set(read, entry);
  entry.graphs.then(
    () => void (entry.landed = now()),
    () => void (indexes.get(read) === entry && indexes.delete(read)),
  );
  return entry.graphs;
}

/** What one `umx resolve umx:<alias>` found: the graph id and live alias it led to, umx's refusal, or nothing. */
type Probe = { id: string; to: string } | { refusal: UmxError } | null;

/** The refusals of a probe that are shown as umx gave them (graph-addressing expert, plan 215). */
const PROBE_REFUSALS = new Set(["former_alias_ambiguous", "graph_not_loaded"]);
/** The most aliases a view server ever asks umx about (review R404-2, pm-009's ruling): past it no alias is asked
 * about again, and an unlisted one is refused as not listed. */
export const PROBES_MAX = 4096;

/** One alias's probe: what it found, and the graph index it was asked under. */
type Asked = { probe: Promise<Probe>; index: string; redirected: boolean };

/** The graph index as a probe remembers it: every row's alias, id and state. */
function indexKey(graphs: readonly GraphSummary[]): string {
  return graphs.map((g) => `${g.alias} ${g.id} ${g.state}`).sort().join("\n");
}

/**
 * The aliases this view server asked umx about (`umx resolve umx:<alias>`), for its life (one view session), in
 * memory only, never stored (graph-addressing expert rule 2), shared by concurrent callers, never asked on a timer.
 * Nothing is forgotten to make room (review R404-2): past `max` aliases no new alias is asked about. A redirect is kept
 * for good; any other answer (a refusal, nothing, a failed read) is asked once more only after the graph index changes
 * (pm-009's ruling on R404-2 and N1), so at most once per alias per index.
 */
export class Probes {
  private readonly asked = new Map<string, Asked>();
  /** How many resolves were sent (for tests). */
  sent = 0;

  constructor(private readonly max: number = PROBES_MAX) {}

  ask(read: Reader, alias: string, graphs: readonly GraphSummary[]): Promise<Probe> {
    const index = indexKey(graphs);
    const was = this.asked.get(alias);
    if (was && (was.redirected || was.index === index)) return was.probe;
    if (!was && this.asked.size >= this.max) return Promise.resolve(null);
    this.sent++;
    const asked: Asked = { probe: Promise.resolve(null), index, redirected: false };
    asked.probe = read<Record<string, unknown>>("resolve graph", { graph: alias }).then(
      (env): Probe => {
        const d = env.data ?? {};
        const id = str(d.graph_id);
        const to = str(d.graph);
        if (d.status !== "redirected" || d.redirected_from !== alias || id === null || to === null || !ALIAS.test(to)) return null;
        asked.redirected = true;
        return { id, to };
      },
      (e: unknown): Probe =>
        e instanceof UmxError && PROBE_REFUSALS.has(e.code) ? { refusal: new UmxError(e.code, e.message, e.next, 409) } : null,
    );
    this.asked.set(alias, asked);
    return asked.probe;
  }
}

const PROBES = Symbol.for("umx.ui.probes");
/** The process-wide probes the routes share. */
export function sharedProbes(): Probes {
  const store = globalThis as Record<symbol, Probes | undefined>;
  return (store[PROBES] ??= new Probes());
}

/**
 * The live alias of the graph an unlisted `alias` was renamed from (plan 215), or null. Asked of umx at most once per
 * alias and graph index (Probes): `umx resolve umx:<alias>` answers `redirected` with the graph id and live alias (umx
 * follows a chain in one step; the view never walks one). The graph is matched in the index by that id, never by
 * name, so a later rename of the same graph is followed with no second probe; it must be loaded once and openable
 * here. A refusal umx gave (former_alias_ambiguous, graph_not_loaded) is thrown as it is.
 */
export async function liveAlias(read: Reader, alias: string, graphs: readonly GraphSummary[], probes: Probes): Promise<Renamed | null> {
  const probe = await probes.ask(read, alias, graphs);
  if (probe === null) return null;
  if ("refusal" in probe) throw probe.refusal;
  const loaded = graphs.filter((g) => g.id === probe.id && g.state === "loaded");
  const row = loaded.length === 1 ? loaded[0] : loaded.find((g) => g.alias === probe.to);
  return row && row.openable && row.alias !== alias ? { from: alias, to: row.alias, id: row.id } : null;
}

/** A launch link's graphs the index does not list, followed (plan 215): each former alias's live alias, and umx's
 * refusal for each it refused. Only these names are asked about, each at most once per session (Probes); names that
 * are no alias, and past MAX_OPEN, are left out. */
export async function followNamed(read: Reader, graphs: readonly GraphSummary[], names: readonly string[], probes: Probes = sharedProbes()) {
  const renamed: Renamed[] = [];
  const refused: { alias: string; issue: ApiError }[] = [];
  const asked = [...new Set(names)].filter((a) => ALIAS.test(a) && !graphs.some((g) => g.alias === a)).slice(0, MAX_OPEN);
  for (const alias of asked) {
    try {
      const r = await liveAlias(read, alias, graphs, probes);
      if (r) renamed.push(r);
    } catch (e) {
      if (e instanceof UmxError) refused.push({ alias, issue: { code: e.code, message: e.message, next: e.next } });
      else throw e;
    }
  }
  return { renamed, refused };
}

type EdgeListRow = {
  id: string;
  type: string;
  from: string;
  to: string;
  graph_id?: string | null;
  from_graph?: string | null;
  to_graph?: string | null;
  from_state?: string | null;
  to_state?: string | null;
  auto_added_by?: unknown;
};

/** Where an edge belongs on a one-graph canvas: drawn, or left out with a reason. */
export function edgePlace(e: EdgeListRow, localId: string): "local" | "otherGraph" | "unresolved" {
  // Asked for by name (EDGE_COLUMNS), a column a row leaves out comes back null: read null as absent.
  if (e.from_state != null || e.to_state != null) return "unresolved";
  if (e.from_graph !== localId || e.to_graph !== localId || (e.graph_id != null && e.graph_id !== localId)) {
    return "otherGraph";
  }
  return "local";
}

/** Declarations and symmetric relations share the ontology read already made with snapshots and live rows. */
export async function ontologyOf(read: Reader, graph: string): Promise<{ symmetric: string[]; hubs?: HubDeclaration[] }> {
  const { data } = await read<{ symmetric?: unknown; hubs?: unknown }>("ontology show", { graph });
  const symmetric = Array.isArray(data?.symmetric) ? data.symmetric.filter((r): r is string => typeof r === "string") : [];
  if (data?.hubs === undefined) return { symmetric };
  const bad = () => new UmxError("ui_bad_hubs", "umx returned invalid hub declarations.", `umx ontology show --graph ${graph}`, 502);
  if (!Array.isArray(data.hubs)) throw bad();
  const hubs = data.hubs.map((row): HubDeclaration => {
    if (row === null || typeof row !== "object" || Array.isArray(row)) throw bad();
    const r: Record<string, unknown> = row;
    if (typeof r.name !== "string" || !r.name || typeof r.via !== "string" || !r.via) throw bad();
    if (r.form === "named" && typeof r.node === "string" && /^[0-9A-Z]{26}$/.test(r.node)) return { name: r.name, form: r.form, via: r.via, node: r.node };
    if (r.form === "type" && typeof r.type === "string" && r.type) return { name: r.name, form: r.form, via: r.via, type: r.type };
    throw bad();
  });
  return { symmetric, hubs };
}

export async function loadSnapshot(read: Reader, graph: string): Promise<Snapshot> {
  type N = { id: string; type: string; title: string; status: string; updated?: unknown; entity_key?: unknown };
  const { localId } = await graphIds(read, graph);
  // The three reads are independent: run together, a snapshot waits for the slowest, not the sum.
  const [nodes, ontology, edges] = await Promise.all([
    pages<N>(read, "node list", graph, NODE_CAP),
    ontologyOf(read, graph),
    pages<EdgeListRow>(read, "edge list", graph, EDGE_CAP),
  ]);
  const omittedEdges = { otherGraph: 0, unresolved: 0 };
  const local: Snapshot["edges"] = [];
  const external: ExternalRow[] = [];
  const broken: string[] = [];
  for (const e of edges.rows) {
    const place = edgePlace(e, localId);
    if (place === "local") {
      const row: EdgeRow = [e.id, e.type, e.from, e.to];
      if (typeof e.auto_added_by === "string") row.push(e.auto_added_by);
      local.push(row);
    }
    else omittedEdges[place]++;
    // The browser draws these only when both graphs are open on its canvas (plan 133): graph ids, never an alias. An
    // end umx gave as graph_not_loaded (a graph not loaded, or a secured one this read did not name) comes too, so a
    // secured graph the person opened by name shows its links; any other state can never be drawn.
    const notLoaded = (s: string | null | undefined) => s == null || s === "graph_not_loaded";
    if (place === "otherGraph" || (place === "unresolved" && notLoaded(e.from_state) && notLoaded(e.to_state))) {
      const row: ExternalRow = [e.id, e.type, e.from_graph ?? localId, e.from, e.to_graph ?? localId, e.to, place === "unresolved"];
      if (typeof e.auto_added_by === "string") row.push(e.auto_added_by);
      external.push(row);
    } else if (place === "unresolved") broken.push(e.id);
  }
  // Entities (ws009 M2): every node keeps its updated time, as any node can be a member through an edge another graph
  // stores (review B4); keys go only on the nodes that hold some, for the entity builder (never shown from
  // here). A title umx flags (F188) leaves as "<type> · <short id>" (ws009 M2b, security).
  const row = (n: N): NodeRow => {
    const keys = keysOf(n.entity_key);
    const updated = str(n.updated) ?? "";
    const title = shownTitle(n as Record<string, unknown>, n.id);
    return keys.length > 0 ? [n.id, n.type, title, n.status, updated, keys] : [n.id, n.type, title, n.status, updated];
  };
  return {
    graph,
    graphId: localId,
    nodes: nodes.rows.map(row),
    edges: local,
    external,
    broken,
    ...ontology,
    totalNodes: nodes.total,
    totalEdges: edges.total,
    truncated: nodes.rows.length < nodes.total || edges.rows.length < edges.total,
    omittedEdges,
    warnings: [...nodes.warnings, ...edges.warnings],
    hints: [...nodes.hints, ...edges.hints],
  };
}

type NeighbourRow = Record<string, unknown>;

/** Types a raw `umx neighbours` row: a resolved node, or an endpoint state with no type, title or status. */
export function toNeighbour(row: NeighbourRow, localId: string, aliasOf: Map<string, string>): Neighbour {
  const graph = str(row.graph) ?? "";
  const base = {
    way: str(row.way) ?? "",
    relation: str(row.relation) ?? "",
    edge: str(row.edge) ?? "",
    id: str(row.id) ?? "",
    graph,
    graphAlias: aliasOf.get(graph) ?? null,
  };
  const state = str(row.state);
  const type = str(row.type);
  if (state !== null || type === null) {
    const candidates = Array.isArray(row.candidates) ? row.candidates.map(String) : [];
    return { ...base, kind: "state", state: state ?? "unresolved", candidates, local: false };
  }
  return {
    ...base,
    kind: "node",
    type,
    title: str(row.title) ?? "",
    status: str(row.status) ?? "",
    local: graph === localId,
  };
}

/** Neighbour titles come from a read that carries no flags: each is taken from a flagged read of the node instead
 * (ws009 M2b, lib/flagged-read). */
export async function vouchNeighbours(read: Reader, rows: Neighbour[]): Promise<Neighbour[]> {
  const nodes = rows.flatMap((n, i) => (n.kind === "node" && n.graphAlias ? [{ i, graph: n.graphAlias, id: n.id, type: n.type, title: n.title }] : []));
  const vouched = await vouchTitles(read, nodes);
  const out = [...rows];
  for (const v of vouched) out[v.i] = { ...(out[v.i] as ResolvedNeighbour), title: v.title };
  // A resolved row whose graph has no alias here cannot be read with flags: its title is not shown.
  return out.map((n) => (n.kind === "node" && !n.graphAlias ? { ...n, title: hiddenTitle(n.type, n.id) } : n));
}

/** `own`: only the node's own edges (`--entities off`), none inherited from its entity's members. */
export async function loadNode(read: Reader, graph: string, id: string, own = false): Promise<NodeDetail> {
  const { localId, aliasOf } = await graphIds(read, graph);
  const got = await read<{ node: Record<string, unknown> }>("node get", { graph, id });
  const near = await read<Rows<NeighbourRow>>(own ? "neighbours own" : "neighbours", { graph, id });
  // Every value umx flags is replaced here, from this reply's own flags (ws009 M2b); keys it does not flag are shown.
  const { node, titleHidden } = redactNode(got.data.node);
  return {
    node,
    titleHidden,
    neighbours: await vouchNeighbours(read, near.data.rows.map((r) => toNeighbour(r, localId, aliasOf))),
    neighboursTruncated: near.data.truncated,
  };
}

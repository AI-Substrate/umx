// The UI's one adapter to umx (architect ruling, ws009): run the installed `umx` with execFile. A fixed absolute
// path resolved once, no shell, a fixed env, fixed argument arrays, ids and aliases checked before use, and only
// the read verbs below. This is the only module allowed to import node:child_process or node:net (eslint).
import { execFile, execFileSync } from "node:child_process";
import { homedir } from "node:os";
import { isAbsolute } from "node:path";
import { MAX_OPEN } from "@/lib/graph/keys";

/** The protocol this view speaks: umx_protocol::PROTOCOL (the umx-arch test protocol_pin keeps the two equal). The
 * socket client sends it, and the daemon refuses any other (protocol_mismatch); CLI reads newer than it warn. */
export const UMX_PROTOCOL = 105;
/** The oldest protocol the view still reads. Older is refused; newer than UMX_PROTOCOL is read with a warning,
 * since a protocol bump usually only adds fields the view ignores. */
export const UMX_PROTOCOL_MIN = 25;
/** The first protocol whose `secret_suspected` flags count list positions from 1 (F217). */
export const FLAGS_FROM_ONE = 76;

const LIST_FLAG = /^(key|evidence|status_reason|status_evidence|evidence_removed):(\d+)$/;

/** Where umx puts the flags it computes in a reply's data (F188): on the data itself (entity show's `keys`), on
 * `node` (node get) and on each entry of `rows`, `members` and `proposed`. Nothing below these records is umx's own: a
 * node's custom field may hold any JSON, a `secret_suspected` key included (review on #385). */
const FLAGGED_LISTS = ["rows", "members", "proposed"] as const;

/** One record's flags, if umx put any there, from 0 to 1 (`key:0` becomes `key:1`), in place. */
function shiftFlags(record: unknown): void {
  if (typeof record !== "object" || record === null || Array.isArray(record)) return;
  const obj = record as Record<string, unknown>;
  const flags = obj.secret_suspected;
  if (!Array.isArray(flags)) return;
  obj.secret_suspected = flags.map((f) => {
    const m = typeof f === "string" ? LIST_FLAG.exec(f) : null;
    return m ? `${m[1]}:${Number(m[2]) + 1}` : f;
  });
}

/** An older umx's flags (`key:0` for the first key) as this view reads them, from 1, so a value an older umx flags is
 * still the one hidden: the list positions in the flags of each record umx flags in `data`, and nowhere else. */
export function flagsFromOne(data: unknown): void {
  if (typeof data !== "object" || data === null) return;
  const obj = data as Record<string, unknown>;
  shiftFlags(obj);
  shiftFlags(obj.node);
  for (const list of FLAGGED_LISTS) {
    const items = obj[list];
    if (Array.isArray(items)) items.forEach(shiftFlags);
  }
}

/** Rows per `node list` / `edge list` page; pages are fetched one at a time. */
export const PAGE = 10_000;

const ID = /^[0-9A-Z]{26}$/;
export const ALIAS = /^[a-z][a-z0-9_-]{0,63}$/;
const TIMEOUT_MS = 60_000;
const MAX_BUFFER = 256 * 1024 * 1024;

export type UmxIssue = { code: string; message: string; next?: string | null };
export type Envelope<T = unknown> = {
  umx: number;
  command: string;
  /** `warning`: it worked, with warnings (umx docs get output); only `error` is a failure. */
  status: "ok" | "warning" | "error";
  data: T;
  warnings: UmxIssue[];
  errors: UmxIssue[];
  /** umx's hints, when the envelope carries them (D69 (9)): shown as given, never made up by the view. */
  hints?: unknown;
};

/** An envelope's `hints` as issues: each a string, or an object with a message (and maybe a code and next). Anything
 * else is left out, and so is `alias_redirected`: the view follows a rename itself (lib/snapshot liveAlias) and says so
 * once, never as a hint card per read (plan 215). */
export function hintsOf(env: { hints?: unknown }): UmxIssue[] {
  if (!Array.isArray(env.hints)) return [];
  const out: UmxIssue[] = [];
  for (const h of env.hints.slice(0, 20)) {
    if (typeof h === "string") out.push({ code: "hint", message: h, next: null });
    else if (h && typeof h === "object" && typeof (h as UmxIssue).message === "string") {
      const o = h as Record<string, unknown>;
      if (o.code === REDIRECTED) continue;
      out.push({ code: typeof o.code === "string" ? o.code : "hint", message: String(o.message), next: typeof o.next === "string" ? o.next : null });
    }
  }
  return out;
}

/** An envelope's `warnings` as the view shows them, `alias_redirected` left out as in hintsOf (plan 215, review
 * R404-3): umx sends it as a hint, and a warning carrying it would be a card per read all the same. */
export function warningsOf(env: { warnings?: readonly UmxIssue[] }): UmxIssue[] {
  const text = (v: unknown) => (typeof v === "string" ? v : "");
  return (env.warnings ?? []).filter((w) => w.code !== REDIRECTED).map((w) => ({ code: text(w.code), message: text(w.message), next: w.next ?? null }));
}

/** umx's hint that a request named a former alias (`umx graph rename`). The view learns the live alias from
 * `umx resolve` instead (lib/snapshot liveAlias), never from this hint's text. */
const REDIRECTED = "alias_redirected";

/** A graph the view asked for by a former alias (plan 215): its live alias and graph id. */
export type Renamed = { from: string; to: string; id: string };

export class UmxError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly next: string | null = null,
    readonly httpStatus = 502,
    readonly renamed: Renamed | null = null,
  ) {
    super(message);
  }
  toJSON(): UmxIssue & { renamed?: Renamed } {
    const issue = { code: this.code, message: this.message, next: this.next };
    return this.renamed ? { ...issue, renamed: this.renamed } : issue;
  }
}

export type ReadArgs = {
  graph?: string;
  id?: string;
  offset?: number;
  query?: string;
  limit?: number;
  ids?: readonly string[];
  /** Other open graphs an edge read also names, so umx follows the external edges between them (plan 133). */
  with?: readonly string[];
  /** A model key (`<model>@<dims>[+<form>]`) a similarity read compares in (ws009 M1c). */
  model?: string;
  /** A declared spine's name (`umx graph spine show`), for its placements. */
  spine?: string;
};

/** A declared spine's name as umx writes one (lowercase, digits, `_`), never a flag or a built-in's `date_field:`. */
const SPINE = /^[a-z][a-z0-9_]{0,63}$/;
function spineName(value: string | undefined): string {
  if (typeof value !== "string" || !SPINE.test(value)) {
    throw new UmxError("ui_bad_spine", "That is not a declared spine's name.", null, 400);
  }
  return value;
}

const MODEL_KEY = /^[A-Za-z0-9._-]{1,100}@[0-9]{1,5}(\+[a-z0-9]{1,16})?$/;
function modelKey(value: string): string {
  if (!MODEL_KEY.test(value)) throw new UmxError("ui_bad_model", "That is not a model key.", null, 400);
  return value;
}

/** The most ids one id-list read takes: the change feed's cap per list (umx-core ui MAX_CHANGE_IDS). */
export const IDS_MAX = 500;
/** The columns the view reads of a node row (`node list`, `node get --columns`): each reply flags what it shows
 * (F188), so a title or key comes with its own flags (ws009 M2b). */
export const NODE_COLUMNS = "id,type,title,status,updated,entity_key";
/** The edge columns a snapshot reads: an edge's ends and where they are, never its titles or sentence, which the view
 * reads per node when it needs them (a full row is about 2.5 times the bytes). */
export const EDGE_COLUMNS = "id,type,from,to,graph_id,from_graph,to_graph,from_state,to_state";
/** Edges by id. The subset has no id binding on a relationship, so this scans the edges once per id (lib/rows sizes
 * the reads to the work budget). The ends' graphs (aliases) say whether the edge is drawn: both open on the canvas.
 * Naming the other open graphs too (`--graph a,b`) is what lets umx follow an external edge at all. */
export const EDGES_BY_ID =
  "MATCH (a)-[r]->(b) WHERE r.id IN $ids AND r.graph = $graph RETURN r.id AS id, r.graph AS owner, label(r) AS type, a.id AS from, b.id AS to, a.graph AS from_graph, b.graph AS to_graph";

/** Shadows per node the view reads (ws009 M1c): Selected shows the top 5 of what it may draw, so it reads 10, as umx
 * cuts to k before the view drops linked, same-entity and hidden ends (embeddings-expert E1). */
export const SHADOW_K = 10;
/** Rows per `similarity pairs` call: umx's most since ws004 s34a (usage 177), so up to 25k nodes at k 10 come in one
 * call (each page recomputes every pair). Paging past it stays as a fallback (lib/shadows-read). */
export const SHADOW_PAGE = 250_000;

/** The longest search text the view sends, in characters. */
export const QUERY_MAX = 200;
/** The most hits a search or title find asks for. */
export const SEARCH_LIMIT_MAX = 50;
/** The most similar_to rows asked for: a node lists at most k (10 by default) of its own. */
export const SIMILAR_LIMIT = 100;
/** C0 and C1 controls, DEL, and the Unicode line and paragraph separators. */
function hasControl(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x20 || (c >= 0x7f && c <= 0x9f) || c === 0x2028 || c === 0x2029) return true;
  }
  return false;
}
const WORD = /[\p{L}\p{N}]/u;
/** Single-dash words umx reads as its own whatever their place: `-h` is help anywhere (crates/umx-cli/src/lib.rs
 * help); `-V` is version as the first word only, refused too in case that widens. Their `--` forms are refused
 * with every other `--` text, and other `-x` words are plain text to umx. */
const UMX_GLOBAL_TOKENS = new Set(["-h", "-V"]);

export function alias(value: string | undefined): string {
  if (typeof value !== "string" || !ALIAS.test(value)) {
    throw new UmxError("ui_bad_graph", "That is not a graph alias this view can open.", "umx graph list", 400);
  }
  return value;
}

export function nodeId(value: string | undefined): string {
  if (typeof value !== "string" || !ID.test(value)) {
    throw new UmxError("ui_bad_id", "That is not a node id (26 Crockford characters).", null, 400);
  }
  return value;
}

function offset(value: number | undefined): string {
  const n = value ?? 0;
  if (!Number.isSafeInteger(n) || n < 0) {
    throw new UmxError("ui_bad_offset", "The page offset must be a whole number of 0 or more.", null, 400);
  }
  return String(n);
}

/**
 * Search text as one argv element that umx reads as words, never as a flag. umx has no `--` terminator: its parser
 * takes any word starting with "--" as a flag, and "--graph=x" as that flag with a value (crates/umx-cli/src/parse.rs,
 * Args::allowing), so text starting with "--" is refused. A single leading "-" is a plain word to umx. Whitespace
 * runs become one space, as a shell splits `umx search a  b` and umx joins the words back.
 */
export function queryText(value: string | undefined): string {
  const bad = (message: string) => new UmxError("ui_bad_query", message, null, 400);
  if (typeof value !== "string") throw bad("Give the words to look for.");
  if (value.length > QUERY_MAX * 4) throw bad(`Search text is at most ${QUERY_MAX} characters.`);
  if (hasControl(value)) throw bad("Search text cannot hold control characters (tabs, new lines and the like).");
  const text = value.trim().split(/\s+/u).join(" ");
  if (!WORD.test(text)) throw bad("Give the words to look for: at least one letter or digit.");
  if ([...text].length > QUERY_MAX) throw bad(`Search text is at most ${QUERY_MAX} characters.`);
  if (text.startsWith("--")) throw bad("Search text cannot start with two dashes: umx would read it as a flag.");
  if (UMX_GLOBAL_TOKENS.has(text)) throw bad(`umx reads "${text}" on its own as its help or version flag; add a word.`);
  return text;
}

/** An id list as umx's `--param-json ids=[...]`: 1 to IDS_MAX ids, each a full id (never a prefix or an address),
 * so the element is JSON umx parses and never a flag. */
function idList(value: readonly string[] | undefined): string {
  if (!Array.isArray(value) || value.length === 0 || value.length > IDS_MAX) {
    throw new UmxError("ui_bad_ids", `An id list holds 1 to ${IDS_MAX} ids.`, null, 400);
  }
  return `ids=${JSON.stringify(value.map((v) => nodeId(v)))}`;
}

/** `--graph` for a read across open graphs: the graph first, then up to MAX_OPEN - 1 others, each a checked alias,
 * each once, joined by commas (umx's graph list). */
function graphList(graph: string | undefined, others: readonly string[] | undefined): string {
  const list = [alias(graph), ...(others ?? []).map((g) => alias(g))];
  if (list.length > MAX_OPEN) throw new UmxError("ui_bad_graph", `A read names at most ${MAX_OPEN} graphs.`, null, 400);
  return [...new Set(list)].join(",");
}

function limit(value: number | undefined): string {
  const n = value ?? 20;
  if (!Number.isSafeInteger(n) || n < 1 || n > SEARCH_LIMIT_MAX) {
    throw new UmxError("ui_bad_limit", `The limit is a whole number from 1 to ${SEARCH_LIMIT_MAX}.`, null, 400);
  }
  return String(n);
}

/** The read-only verbs the UI may run, each a fixed argument array. Nothing else reaches execFile. */
const READ_VERBS = {
  // This machine's graph index (req-116): the loaded graphs, then the remembered ones, each with its state.
  "graph list": () => ["graph", "list", "--all"],
  // Where a graph address now leads (plan 215): `umx:<alias>` with no id names the graph itself, and a former alias
  // answers `status: redirected` with `redirected_from`, the live alias (`graph`) and `graph_id`. Asked at most once
  // per alias, only for one the index does not list (lib/snapshot liveAlias).
  "resolve graph": (a: ReadArgs) => ["resolve", `umx:${alias(a.graph)}`],
  // Saved scopes and their members by graph id (plan 133); the picker opens a scope's open members.
  "scope list": () => ["scope", "list"],
  // The graph's vocabulary: the view reads only its symmetric relations, drawn without arrowheads (plan 150).
  "ontology show": (a: ReadArgs) => ["ontology", "show", "--graph", alias(a.graph)],
  "node list": (a: ReadArgs) => [
    "node", "list", "--graph", alias(a.graph), "--limit", String(PAGE), "--offset", offset(a.offset),
    "--columns", NODE_COLUMNS,
  ],
  "edge list": (a: ReadArgs) => [
    "edge", "list", "--graph", alias(a.graph), "--limit", String(PAGE), "--offset", offset(a.offset),
    "--columns", EDGE_COLUMNS,
  ],
  "node get": (a: ReadArgs) => ["node", "get", nodeId(a.id), "--graph", alias(a.graph)],
  // The graph's declared spines (spine view): each with its decl, clock and cover. Reads only.
  "spine show": (a: ReadArgs) => ["graph", "spine", "show", alias(a.graph)],
  // Where one declared spine places each node (`--columns spine:<name>`): its band in the spine's order and how it was
  // placed, by umx's one placement rule, so the view and the CLI agree.
  "spine places": (a: ReadArgs) => [
    "node", "list", "--graph", alias(a.graph), "--limit", String(PAGE), "--offset", offset(a.offset),
    "--columns", `id,spine:${spineName(a.spine)}`,
  ],
  // One node's row with its flags (ws009 M2b): live rows and the titles of reads that carry no flags (lib/flagged-read).
  "node row": (a: ReadArgs) => ["node", "get", nodeId(a.id), "--graph", alias(a.graph), "--columns", NODE_COLUMNS],
  // A node's entity (ws009 M2, `umx entity show`): over the open graphs it names (the node's own first), by address,
  // as umx asks when a list is named; and over every loaded open graph, for members in graphs that are not open. A
  // secured graph is covered only when named, so one that is not open is never counted.
  // umx covers exactly the named graphs only when the list has a comma; one graph alone would widen to every loaded
  // open graph, so a lone open graph is named twice (review B2).
  "entity show": (a: ReadArgs) => {
    const list = graphList(a.graph, a.with);
    return ["entity", "show", `umx:${alias(a.graph)}/${nodeId(a.id)}`, "--graph", list.includes(",") ? list : `${list},${list}`];
  },
  "entity show loaded": (a: ReadArgs) => ["entity", "show", nodeId(a.id), "--graph", alias(a.graph)],
  neighbours: (a: ReadArgs) => ["neighbours", nodeId(a.id), "--graph", alias(a.graph), "--limit", "1000"],
  // The search text always goes right after the verb's words, as one element; queryText makes sure it is no flag.
  "node find": (a: ReadArgs) => ["node", "find", queryText(a.query), "--graph", alias(a.graph), "--limit", limit(a.limit)],
  search: (a: ReadArgs) => ["search", queryText(a.query), "--graph", alias(a.graph), "--limit", limit(a.limit)],
  // The node's own similar_to list (its top k by whole-node vector, best first), read from the daemon: vectors never
  // leave it (usage 081).
  "similar to": (a: ReadArgs) => [
    "neighbours", nodeId(a.id), "--graph", alias(a.graph), "--relation", "similar_to", "--direction", "out",
    "--limit", String(SIMILAR_LIMIT),
  ],
  // Shadow connections (ws009 M1c): each node's top SHADOW_K by whole-node vector over exactly the open graphs, inside
  // each graph and across them, at the model's own floor (no --min). A lone open graph is named twice so umx reads
  // exactly it (review C2). Lean rows (`--rows ids`, usage 182): graph, id, other graph, other id and score only, the
  // fields the view reads. Read only: it embeds nothing.
  "similarity pairs": (a: ReadArgs) => {
    const list = graphList(a.graph, a.with);
    return [
      "similarity", "pairs", "--graph", list.includes(",") ? list : `${list},${list}`, "--same-graph", "--k", String(SHADOW_K),
      "--limit", String(SHADOW_PAGE), "--offset", offset(a.offset), "--rows", "ids",
      ...(a.model !== undefined ? ["--model", modelKey(a.model)] : []),
    ];
  },
  // Each loaded graph's vectors_digest (usage 182), the key the shadows cache is kept by (plan 184). It sends nothing
  // and reads no credential; the server reads only the open graphs' rows and none of it reaches the browser.
  "embeddings status": () => ["embeddings", "status"],
  // The daemon's pid, start time and build, so the shadows cache drops what an earlier daemon answered (a floor or
  // calibration change takes a restart; embeddings-expert note). It reads what the daemon holds in memory and
  // sends nothing; the server keeps only those three fields and none of it reaches the browser.
  "daemon status": () => ["daemon", "status"],
  // Live updates (plan 127): the edges a change touched, by id (nodes are read with their flags, "node row" and
  // "node list"). The query text is fixed; the ids go in a parameter, never into the text.
  // Only edges the graph itself stores (`r.graph = $graph`, before the limit): with other graphs named, an equal edge
  // id another graph stores is never read as this graph's (review B2).
  "edges by id": (a: ReadArgs) => [
    "query", EDGES_BY_ID, "--graph", graphList(a.graph, a.with), "--param-json", idList(a.ids), "--param-json",
    `graph=${JSON.stringify(alias(a.graph))}`, "--limit", String(IDS_MAX),
  ],
} as const satisfies Record<string, (a: ReadArgs) => string[]>;

export type ReadVerb = keyof typeof READ_VERBS;
export const READ_VERB_NAMES = Object.keys(READ_VERBS) as ReadVerb[];

export type Exec = (
  file: string,
  args: string[],
  env: Record<string, string>,
) => Promise<{ stdout: string; failed: boolean; detail: string }>;

const execUmx: Exec = (file, args, env) =>
  new Promise((resolve) => {
    execFile(
      file,
      args,
      { env: env as unknown as NodeJS.ProcessEnv, shell: false, timeout: TIMEOUT_MS, maxBuffer: MAX_BUFFER, encoding: "utf8", windowsHide: true },
      (error, stdout, stderr) => {
        resolve({ stdout: String(stdout ?? ""), failed: error != null, detail: String(stderr || error?.message || "") });
      },
    );
  });

export type UmxDeps = { exec: Exec; bin: () => string; env: () => Record<string, string> };

/** Where `umx` lives: UMX_BIN (absolute) or the first `umx` on PATH, resolved once. */
export function resolveBin(env: Record<string, string | undefined>, which: (path: string) => string | null): string {
  const given = env.UMX_BIN;
  if (given !== undefined && given !== "") {
    if (!isAbsolute(given)) throw new UmxError("ui_umx_bin", "UMX_BIN must be an absolute path.", null, 500);
    return given;
  }
  const found = which(env.PATH ?? "");
  if (!found || !isAbsolute(found)) {
    throw new UmxError("ui_umx_missing", "umx is not on PATH. Install umx or set UMX_BIN to its absolute path.", null, 500);
  }
  return found;
}

/** The only environment umx gets: its home and JSON output. */
export function umxEnv(env: Record<string, string | undefined>): Record<string, string> {
  const home = env.UMX_HOME && env.UMX_HOME !== "" ? env.UMX_HOME : `${homedir()}/.umx`;
  return { UMX_HOME: home, UMX_OUTPUT: "json" };
}

let warnedNewer = false;

export function parseEnvelope<T>(stdout: string, failed: boolean, detail: string): Envelope<T> {
  let envelope: Envelope<T>;
  try {
    envelope = JSON.parse(stdout) as Envelope<T>;
  } catch {
    // execFile's error message and stderr can hold the argv, search text included: they go to the server's
    // terminal only, never to the browser (security N2).
    if (failed) console.error(`umx failed: ${detail.slice(0, 300)}`);
    throw new UmxError("ui_umx_output", failed ? "umx failed; see the view's terminal." : "umx did not answer with JSON.");
  }
  if (typeof envelope !== "object" || envelope === null || typeof envelope.umx !== "number") {
    throw new UmxError("ui_umx_output", "umx answered without an envelope.");
  }
  if (envelope.umx < UMX_PROTOCOL_MIN) {
    throw new UmxError(
      "ui_protocol_mismatch",
      `This view needs umx protocol ${UMX_PROTOCOL_MIN} or newer; the installed umx speaks ${envelope.umx}. Update umx.`,
      "umx --version",
      500,
    );
  }
  if (envelope.umx > UMX_PROTOCOL) {
    const warning = {
      code: "ui_protocol_newer",
      message: `This view was tested with umx protocol ${UMX_PROTOCOL}; the installed umx speaks ${envelope.umx}. Reading anyway.`,
      next: null,
    };
    if (!warnedNewer) {
      warnedNewer = true;
      console.warn(warning.message);
    }
    envelope.warnings = [...(envelope.warnings ?? []), warning];
  }
  if (envelope.umx < FLAGS_FROM_ONE) flagsFromOne(envelope.data);
  // An error anywhere fails the read, whatever status says (review R199-2).
  if ((envelope.status !== "ok" && envelope.status !== "warning") || (envelope.errors?.length ?? 0) > 0) {
    const first = envelope.errors?.[0];
    throw new UmxError(first?.code ?? "umx_error", first?.message ?? "umx refused the request.", first?.next ?? null);
  }
  return envelope;
}

export function createUmx(deps: UmxDeps) {
  let bin: string | null = null;
  return async function read<T = unknown>(verb: string, args: ReadArgs = {}): Promise<Envelope<T>> {
    if (!Object.hasOwn(READ_VERBS, verb)) {
      throw new UmxError("ui_verb_refused", `The view only reads; "${verb}" is not one of its verbs.`, null, 400);
    }
    const argv = READ_VERBS[verb as ReadVerb](args);
    bin ??= deps.bin();
    const { stdout, failed, detail } = await deps.exec(bin, argv, deps.env());
    return parseEnvelope<T>(stdout, failed, detail);
  };
}

function whichUmx(path: string): string | null {
  try {
    const out = execFileSync("/usr/bin/which", ["umx"], { env: { PATH: path } as unknown as NodeJS.ProcessEnv, encoding: "utf8", shell: false, timeout: 5_000 });
    return out.trim().split("\n")[0] || null;
  } catch {
    return null;
  }
}

let shared: ReturnType<typeof createUmx> | null = null;

/** The process-wide reader, wired to the real umx. */
export function umx(): ReturnType<typeof createUmx> {
  shared ??= createUmx({
    exec: execUmx,
    bin: () => resolveBin(process.env, whichUmx),
    env: () => umxEnv(process.env),
  });
  return shared;
}

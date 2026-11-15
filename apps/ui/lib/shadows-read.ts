// Shadow connections (ws009 M1c, plan 169): what umx finds similar but nobody linked, inside each open graph and across
// them, from `umx similarity pairs` over exactly the open graphs (security's scope rules on M1c): every row is kept only
// when both its ends are in an open graph, and only ids, aliases and the score leave the server, never a title, field,
// vector or key. A model is named by its short name only. Read only: nothing is embedded. One call compares one model
// key, so open graphs on another key are read in a call of their own (embeddings-expert E2). With a cache and
// a model key from `umx embeddings status` for every open graph, the first calls are one per key, each naming it with
// `--model` (plan 193): with every graph in one call, umx skips the other keys' graphs, so that call could never be
// kept. A read stops between pages once the browser has gone (review B4).
// Rows come as `--rows ids` (graph, id, other graph, other id, score; usage 182). Each call's answer is kept by the
// daemon that answered (pid, start time, build), the open set, its model key, and each of its graphs' `vectors_digest`
// (plan 184, F194): a read whose key stands answers from what was kept, and a moved digest reads the pairs again, even
// when node and vector counts stayed the same (review B2). Whether a key stands is asked of `umx embeddings status`
// before the read; what a read anew is kept under is the digests its own reply reports in `compared`, taken with the
// vectors it read (plan 189, F206), so an edit and its undo inside one read (A, B, A) keep B's pairs under B. A call is
// kept only when every graph it names is in `compared` with a digest: one umx skipped (another model, no vectors yet)
// is not, so that call always reads. The digest is umx's stamp of each vector's model key, node and text hash, not of
// the vector bytes, so what is kept is as fresh as that stamp says (usage 182). A call naming a graph that status
// leaves without a digest (secured, no vectors, unknown counts, missing from status) is never kept either: that call
// always reads, whatever `compared` says (security on plan 189: umx names a secured graph's digest in `compared` when
// the call names it).
import { MAX_OPEN } from "@/lib/graph/keys";
import { SHADOW_ROWS_MAX } from "@/lib/graph/shadows";
import { alias as checkAlias, type createUmx, UmxError } from "@/lib/umx";

type Reader = ReturnType<typeof createUmx>;
type Row = Record<string, unknown>;

/** [alias, id, other alias, other id, score, group]: one direction of a pair, as umx gives it; `group` indexes
 * ShadowReply.groups (the model key it was compared in). */
export type ShadowRow = [string, string, string, string, number, number];
/** One model key's call: its short name, and its floor when calibrated (null: ranked only). */
export type ShadowGroup = { model: string | null; floor: number | null; graphs: string[] };
export type ShadowReply = {
  rows: ShadowRow[];
  groups: ShadowGroup[];
  /** Open graphs umx did not compare in any call, with the model's short name: no vectors yet, or another model when
   * a graph's key moved, after status, into a model this read had already asked (plan 193). */
  skipped: { graph: string; reason: string; model: string | null }[];
  /** umx refused: too many to compare (similarity_pairs_too_large). */
  tooLarge?: true;
  /** More rows than the view reads (SHADOW_ROWS_MAX). */
  cut?: true;
};

const str = (v: unknown) => (typeof v === "string" ? v : "");
/** `embeddinggemma-2@256+p1` -> `embeddinggemma-2`. */
export const shortModel = (m: unknown) => (typeof m === "string" && m !== "" ? m.split("@")[0] : null);

function stopped(signal: AbortSignal | undefined) {
  if (signal?.aborted) throw new UmxError("ui_read_cancelled", "The view no longer wants this read.", null, 499);
}

/** umx's refusal of a call whose graphs have no vectors yet (usage_invalid, "no graph in scope has vectors yet"). */
const noVectors = (e: unknown) => e instanceof UmxError && e.code === "usage_invalid" && e.message.includes("no graph in scope has vectors yet");

/** One `similarity pairs` call: the first graph, the others it names, and the model key it compares in (none: umx's
 * pick, the first graph's). */
type Job = { graphs: string[]; model?: string };
/** [alias, id, other alias, other id, score]: a lean row with both ends open. */
type Lean = [string, string, string, string, number];
/** What a call's first page says beyond its rows (review N2: only this is kept, never the page): its model
 * key, its floor when calibrated, the open graphs it compared (null: umx did not say), and the open graphs it left out
 * with their model and reason. */
type Meta = { model: string | null; floor: number | null; graphs: string[] | null; skipped: { graph: string; model: string; reason: string }[] };
/** What one call gave: its first page's Meta (null when umx refused it as no vectors yet), its rows, and whether it
 * stopped at SHADOW_ROWS_MAX. */
type Got = { meta: Meta | null; rows: Lean[]; cut: boolean };
/** Each graph the call compared with the `vectors_digest` umx took with the vectors it read (`compared`, F206); a
 * graph listed with no digest is left out. Null when a page lacks the list or the pages disagree (each page compares
 * again), so the call is never kept by it. Not kept with the answer: it only makes the key. */
type Compared = Map<string, string> | null;

function comparedOf(page: Row): Compared {
  if (!Array.isArray(page.compared)) return null;
  const digests = new Map<string, string>();
  for (const c of page.compared as Row[]) {
    const digest = c?.vectors_digest;
    if (typeof digest === "string" && digest !== "") digests.set(str(c.graph), digest);
  }
  return digests;
}

function metaOf(page: Row, open: ReadonlyMap<string, string>): Meta {
  const min = (page.min ?? {}) as Row;
  const skipped = Array.isArray(page.skipped_model) ? (page.skipped_model as Row[]) : [];
  return {
    model: typeof page.model === "string" ? page.model : null,
    // An uncalibrated key comes back with a floor of 0: ranked only (embeddings-expert N3).
    floor: min.calibrated === true && typeof min.value === "number" ? min.value : null,
    graphs: Array.isArray(page.graphs) ? (page.graphs as unknown[]).map(str).filter((g) => open.has(g)) : null,
    skipped: skipped.map((s) => ({ graph: str(s.graph), model: str(s.model), reason: str(s.reason) })).filter((s) => open.has(s.graph)),
  };
}

/** One call, paging only as a fallback (one page holds SHADOW_PAGE rows). Aliases are the open set's own strings and
 * each id is kept once, so a kept answer holds a string per node, not four per row. */
async function readJob(read: Reader, job: Job, open: ReadonlyMap<string, string>, signal: AbortSignal | undefined): Promise<{ got: Got; compared: Compared }> {
  const [graph, ...others] = job.graphs;
  const ids = new Map<string, string>();
  const once = (v: unknown) => {
    const s = str(v);
    const had = ids.get(s);
    if (had !== undefined) return had;
    ids.set(s, s);
    return s;
  };
  const got: Got = { meta: null, rows: [], cut: false };
  let compared: Compared = null;
  for (let offset = 0; ; ) {
    stopped(signal);
    let page: Row;
    try {
      page = (await read<Row>("similarity pairs", { graph, with: others, offset, ...(job.model ? { model: job.model } : {}) })).data;
    } catch (e) {
      // A read the view no longer wants never becomes an answer, a refusal included (review N2).
      stopped(signal);
      // umx refuses a call none of whose graphs has vectors yet: each is "no vectors yet", not a failed read.
      if (offset === 0 && noVectors(e)) return { got, compared };
      throw e;
    }
    stopped(signal);
    const pageCompared = comparedOf(page);
    if (got.meta === null) {
      got.meta = metaOf(page, open);
      compared = pageCompared;
    } else if (compared === null || pageCompared === null || JSON.stringify([...compared]) !== JSON.stringify([...pageCompared])) compared = null;
    const rows = Array.isArray(page.rows) ? (page.rows as Row[]) : [];
    for (const r of rows) {
      const [a, b] = [open.get(str(r.graph)), open.get(str(r.other_graph))];
      if (a === undefined || b === undefined || typeof r.score !== "number") continue;
      got.rows.push([a, once(r.id), b, once(r.other_id), r.score]);
    }
    offset += rows.length;
    if (got.rows.length >= SHADOW_ROWS_MAX) {
      got.cut = true;
      return { got, compared };
    }
    if (page.truncated !== true || rows.length === 0) return { got, compared };
  }
}

/** What the kept answers are kept by: the daemon that answers (`umx daemon status`' pid, start time and build: a floor
 * or calibration change takes a daemon restart, embeddings-expert note) and each open graph's `vectors_digest`
 * from `umx embeddings status`. A graph status gives no digest is left out, so calls naming it are never looked up:
 * null is a graph with no vectors, or a secured graph, which status never names. A status umx refuses keeps
 * nothing. Graphs not open are never looked at, and nothing read here leaves the server. `keys` is each open graph's
 * model key from the same status reply (graph.json's, as `similarity pairs --model` takes it), which only groups the
 * first calls (plan 193); a secured graph is listed with its key, so it is compared with its key's graphs as before. */
type Stamp = { daemon: string | null; digests: Map<string, string>; keys: Map<string, string> };

async function stampOf(read: Reader, open: ReadonlyMap<string, string>, signal: AbortSignal | undefined): Promise<Stamp> {
  const data = (verb: string) => read<Row>(verb).then((e) => e.data, () => null);
  const [status, daemon] = await Promise.all([data("embeddings status"), data("daemon status")]);
  stopped(signal);
  const digests = new Map<string, string>();
  const keys = new Map<string, string>();
  for (const g of Array.isArray(status?.graphs) ? (status.graphs as Row[]) : []) {
    const alias = str(g?.graph);
    if (!open.has(alias)) continue;
    const digest = g?.vectors_digest;
    if (typeof digest === "string" && digest !== "") digests.set(alias, digest);
    if (typeof g.key === "string" && g.key !== "") keys.set(alias, g.key);
  }
  const known = typeof daemon?.pid === "number" && typeof daemon.started_ms === "number" && daemon.version !== undefined;
  return { daemon: known ? JSON.stringify([daemon!.pid, daemon!.started_ms, daemon!.version]) : null, digests, keys };
}

/** The first calls (plan 193): when status gives every open graph a model key (a secured graph's included) and
 * there are two or more, one call per key naming it with `--model`, its graphs in open-set order. Otherwise one call
 * over every open graph with no model, umx's pick, as before: a graph status gives no key (graph.json records none
 * yet, or it is not listed) may still have vectors in its embedder's model, which only umx's pick finds (review B1
 *). The same holds with no cache or a refused status. */
function firstJobs(open: readonly string[], stamp: Stamp | null): Job[] {
  const byKey = new Map<string, string[]>();
  for (const g of open) {
    const key = stamp?.keys.get(g);
    if (key === undefined) return [{ graphs: [...open] }];
    byKey.set(key, [...(byKey.get(key) ?? []), g]);
  }
  if (byKey.size < 2) return [{ graphs: [...open] }];
  return [...byKey].map(([model, graphs]) => ({ graphs, model }));
}

/** A call's cache key: the daemon, the open set, the call's model key and graphs, and each graph's digest; null when the
 * daemon or any graph has none. */
function keyOf(open: readonly string[], job: Job, stamp: Stamp): string | null {
  if (stamp.daemon === null) return null;
  const parts: string[] = [];
  for (const g of job.graphs) {
    const d = stamp.digests.get(g);
    if (d === undefined) return null;
    parts.push(`${g}=${d}`);
  }
  return JSON.stringify([stamp.daemon, [...open].sort(), job.model ?? "", parts]);
}

/** Calls' answers by key, newest kept: at most CACHE_ENTRIES of them and CACHE_ROWS rows in all (a Hold keeps a
 * second open set's worth; the newest is always kept). */
export const CACHE_ENTRIES = 8;
export const CACHE_ROWS = 2 * SHADOW_ROWS_MAX;
export class ShadowCache {
  private entries = new Map<string, Got>();
  private rows = 0;

  get(key: string): Got | undefined {
    const got = this.entries.get(key);
    if (got !== undefined) {
      this.entries.delete(key);
      this.entries.set(key, got);
    }
    return got;
  }

  put(key: string, got: Got) {
    this.drop(key);
    this.entries.set(key, got);
    this.rows += got.rows.length;
    for (const k of this.entries.keys()) {
      if (k === key || (this.entries.size <= CACHE_ENTRIES && this.rows <= CACHE_ROWS)) break;
      this.drop(k);
    }
  }

  get size() {
    return this.entries.size;
  }

  /** Drops every answer whose open set names `alias` (plan 215: a graph renamed from it). Kept answers are never
   * rekeyed to the live alias: the next read keeps its own. */
  forget(alias: string) {
    for (const key of [...this.entries.keys()]) {
      const [, open] = JSON.parse(key) as [string, string[]];
      if (open.includes(alias)) this.drop(key);
    }
  }

  private drop(key: string) {
    const was = this.entries.get(key);
    if (was === undefined) return;
    this.rows -= was.rows.length;
    this.entries.delete(key);
  }
}

const CACHE = Symbol.for("umx.ui.shadows");
/** The process-wide cache the shadows route reads through. */
export function shadowCache(): ShadowCache {
  const store = globalThis as Record<symbol, ShadowCache | undefined>;
  return (store[CACHE] ??= new ShadowCache());
}

/** The shadows over `graphs` (the open set, or its part with nothing held). With a cache, the stamp (daemon, digests
 * and model keys) is read first, the first calls are one per model key when status gives every open graph one (plan
 * 193), and each call whose key stands answers from it. A call read anew is kept under the digests its own reply
 * reports in `compared`, never a status read's, and only when that stamp gave the call a key: a call naming a secured
 * graph, or a graph status gives no digest, is never kept, even when its reply lists a digest for every graph
 * (security on plan 189). A graph umx still puts in another model (its key moved after status) is read in that
 * model's call: one still queued takes it, one not yet asked is added, and a model already asked is not asked again
 * but named in `skipped`. */
export async function loadShadows(read: Reader, graphs: readonly string[], signal?: AbortSignal, cache?: ShadowCache): Promise<ShadowReply> {
  const open = [...new Set(graphs.map((g) => checkAlias(g)))];
  if (open.length === 0 || open.length > MAX_OPEN) throw new UmxError("ui_bad_graph", `Shadows read 1 to ${MAX_OPEN} open graphs.`, null, 400);
  const inOpen = new Map(open.map((g) => [g, g]));
  const stamp = cache ? await stampOf(read, inOpen, signal) : null;
  const reply: ShadowReply = { rows: [], groups: [], skipped: [] };
  const jobs = firstJobs(open, stamp);
  const models = new Set<string>();
  const skipped = new Map<string, { graph: string; reason: string; model: string | null }>();
  try {
    while (jobs.length > 0) {
      const job = jobs.shift()!;
      const key = stamp ? keyOf(open, job, stamp) : null;
      let got = key === null ? undefined : cache!.get(key);
      if (got === undefined) {
        const anew = await readJob(read, job, inOpen, signal);
        got = anew.got;
        // Kept under the digests the reply reports (plan 189); keyOf keeps nothing when a graph the call names has none
        // there (skipped, null, left out).
        const kept = key === null || anew.compared === null ? null : keyOf(open, job, { ...stamp!, digests: anew.compared });
        if (kept !== null) cache!.put(kept, got);
      }
      stopped(signal);
      const { meta } = got;
      // Nothing compared (no vectors yet): no group for this call.
      if (meta === null) {
        for (const g of job.graphs) skipped.set(g, { graph: g, reason: "no vectors yet", model: null });
        continue;
      }
      // A call naming its model compares nothing when none of its graphs has vectors there yet (review B3):
      // no group, only what it skipped.
      if (meta.graphs === null || meta.graphs.length > 0) {
        const group = reply.groups.length;
        for (const [a, ia, b, ib, score] of got.rows) reply.rows.push([a, ia, b, ib, score, group]);
        if (got.cut || reply.rows.length >= SHADOW_ROWS_MAX) reply.cut = true;
        reply.groups.push({ model: shortModel(meta.model), floor: meta.floor, graphs: meta.graphs ?? job.graphs });
      }
      for (const m of [meta.model, job.model]) if (m) models.add(m);
      const another = new Map<string, string[]>();
      for (const { graph: g, model, reason } of meta.skipped) {
        if (reason !== "another model") skipped.set(g, { graph: g, reason, model: shortModel(model) });
        else if (model === "") continue;
        else if (jobs.find((j) => j.model === model) || !models.has(model)) another.set(model, [...(another.get(model) ?? []), g]);
        // Its key moved into a model already asked (project manager, plan 193): not compared in this read, and said
        // so; the call that skipped it is not kept, so the next read asks again.
        else skipped.set(g, { graph: g, reason, model: shortModel(model) });
      }
      for (const [model, gs] of another) {
        const queued = jobs.find((j) => j.model === model);
        if (queued) queued.graphs.push(...gs.filter((g) => !queued.graphs.includes(g)));
        else {
          models.add(model);
          jobs.push({ graphs: gs, model });
        }
      }
      if (reply.cut) break;
    }
  } catch (e) {
    if (e instanceof UmxError && e.code === "similarity_pairs_too_large") return { rows: [], groups: [], skipped: [], tooLarge: true };
    throw e;
  }
  reply.skipped = [...skipped.values()];
  return reply;
}

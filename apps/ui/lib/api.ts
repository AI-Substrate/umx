import { VIEW_HEADER } from "@/lib/guard";
import type { ApiError } from "@/lib/types";

export class ApiFailure extends Error {
  constructor(readonly status: number, readonly issue: ApiError) {
    super(issue.message);
  }
}

/** A graph the view asked for by an alias it was renamed from (plan 215), as the view's server found it. */
export type GraphRenamed = { from: string; to: string; id: string };

const NAME = /^[a-z][a-z0-9_-]{0,63}$/;
const renameListeners = new Set<(r: GraphRenamed) => void>();

/** Calls `listener` with each `ui_graph_renamed` any read of the view's server answers, whichever part of the view
 * made it; the returned function stops it. The rename lives in memory only (graph-addressing expert rule 2). */
export function onRenamed(listener: (r: GraphRenamed) => void): () => void {
  renameListeners.add(listener);
  return () => {
    renameListeners.delete(listener);
  };
}

/** The rename a refusal names, when it is a well-formed `ui_graph_renamed`. */
export function renamedOf(issue: unknown): GraphRenamed | null {
  if (!issue || typeof issue !== "object" || !("code" in issue) || issue.code !== "ui_graph_renamed" || !("renamed" in issue)) return null;
  const r = issue.renamed;
  if (!r || typeof r !== "object" || !("from" in r) || !("to" in r) || !("id" in r)) return null;
  const { from, to, id } = r;
  if (typeof from !== "string" || typeof to !== "string" || typeof id !== "string") return null;
  return NAME.test(from) && NAME.test(to) && from !== to ? { from, to, id } : null;
}

async function failure(res: Response): Promise<ApiFailure> {
  let issue: ApiError = { code: `http_${res.status}`, message: await res.text().catch(() => res.statusText) };
  try {
    issue = (JSON.parse(issue.message) as { error: ApiError }).error ?? issue;
  } catch {
    // plain text from the proxy
  }
  const renamed = renamedOf(issue);
  if (renamed) for (const l of [...renameListeners]) l(renamed);
  return new ApiFailure(res.status, issue);
}

export async function api<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, { headers: { [VIEW_HEADER]: "1" }, credentials: "same-origin", cache: "no-store", signal });
  if (!res.ok) throw await failure(res);
  return (await res.json()) as T;
}

/** POSTs `body` as JSON to the view's own server. */
export async function post<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", [VIEW_HEADER]: "1" },
    body: JSON.stringify(body),
    credentials: "same-origin",
    cache: "no-store",
    signal,
  });
  if (!res.ok) throw await failure(res);
  return (await res.json()) as T;
}

export function asIssue(e: unknown): ApiError {
  return e instanceof ApiFailure ? e.issue : { code: "ui_fetch", message: String(e) };
}

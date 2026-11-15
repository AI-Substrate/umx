"use client";
import { useEffect, useState } from "react";
import { api, asIssue } from "@/lib/api";
import { qid } from "@/lib/graph/keys";
import type { ApiError, SearchMode, SearchResult } from "@/lib/types";

/** Waits this long after the last key before asking umx. */
const DEBOUNCE_MS = 250;
const WORD = /[\p{L}\p{N}]/u;

export type SearchState = { loading: boolean; result: SearchResult | null; error: ApiError | null };
const IDLE: SearchState = { loading: false, result: null, error: null };

/**
 * One search per open graph, merged (plan 133): each graph's hits keep umx's order and take turns, rank by rank, so no
 * graph's scores are compared with another's. Hit ids become canvas keys (`<alias>/<id>`), and with several graphs open
 * each hit and warning says its graph. A graph whose search failed is named in the error; the others still list.
 */
export function mergeResults(parts: { alias: string; result: SearchResult }[], multi: boolean): SearchResult {
  const first = parts[0]?.result;
  const lists = parts.map(({ alias, result }) => result.hits.map((h) => ({ ...h, id: qid(alias, h.id), ...(multi ? { graph: alias } : {}) })));
  const hits = [];
  for (let rank = 0; lists.some((l) => rank < l.length); rank++) for (const l of lists) if (rank < l.length) hits.push(l[rank]);
  const notes = [...new Set(parts.map((p) => p.result.note).filter((n): n is string => n !== null))];
  // With hits from another graph, a graph that only found nothing gets one quiet line, not a card each (ui-expert N2).
  // With no hits anywhere, each graph's warning shows as before.
  // find_no_match is a hint since protocol 71 (ws002 X1) and a warning before it: read both lists
  // (the project manager).
  const said = (r: SearchResult) => [...r.warnings, ...(r.hints ?? [])];
  const missed = multi && hits.length > 0 ? parts.filter((p) => said(p.result).length > 0 && said(p.result).every((w) => w.code === "find_no_match")).map((p) => p.alias) : [];
  return {
    mode: first?.mode ?? "title",
    query: first?.query ?? "",
    hits,
    warnings: parts.filter((p) => !missed.includes(p.alias)).flatMap(({ alias, result }) => result.warnings.map((w) => (multi ? { ...w, message: `${alias}: ${w.message}` } : w))),
    ...(missed.length > 0 ? { quiet: `No title match in ${missed.join(", ")}` } : {}),
    note: notes.length > 0 ? notes.join(" · ") : null,
    foreign: parts.reduce((n, p) => n + p.result.foreign, 0),
    hints: parts.filter((p) => !missed.includes(p.alias)).flatMap(({ alias, result }) => (result.hints ?? []).map((h) => (multi ? { ...h, message: `${alias}: ${h.message}` } : h))),
  };
}

/** Title find or hybrid search through the view's server, for the text in the box, in every open graph. Only the
 * newest answer counts. `data` moves with each live change (plan 127): the search runs again, keeping its last answer
 * on screen until the new one comes. */
export function useSearch(aliases: readonly string[], mode: SearchMode, query: string, data = 0): SearchState {
  const [state, setState] = useState<{ key: string; value: SearchState } | null>(null);
  const graphs = aliases.join(",");
  const key = `${graphs}\u0000${mode}\u0000${query}`;
  const wanted = graphs !== "" && WORD.test(query);
  useEffect(() => {
    if (!wanted) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      setState((s) => (s && s.key === key && s.value.result ? s : { key, value: { loading: true, result: null, error: null } }));
      const list = graphs.split(",");
      void Promise.allSettled(
        list.map((alias) => api<SearchResult>(`/api/graphs/${encodeURIComponent(alias)}/search?mode=${mode}&q=${encodeURIComponent(query)}`, abort.signal)),
      ).then((settled) => {
        if (abort.signal.aborted) return;
        const parts = settled.flatMap((r, i) => (r.status === "fulfilled" ? [{ alias: list[i], result: r.value }] : []));
        const failed = settled.flatMap((r, i) => (r.status === "rejected" ? [{ alias: list[i], issue: asIssue(r.reason) }] : []));
        const error: ApiError | null =
          failed.length === 0 ? null : list.length === 1 ? failed[0].issue : { ...failed[0].issue, message: `${failed.map((f) => f.alias).join(", ")}: ${failed[0].issue.message}` };
        setState({ key, value: { loading: false, result: parts.length > 0 ? mergeResults(parts, list.length > 1) : null, error } });
      });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [key, wanted, graphs, mode, query, data]);
  if (!wanted) return IDLE;
  return state && state.key === key ? state.value : { loading: true, result: null, error: null };
}

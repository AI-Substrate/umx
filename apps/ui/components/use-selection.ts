"use client";
// Publishes this tab's selection (lib/selection Publisher, plan 117) and says whether it is shared.
import { useCallback, useEffect, useMemo, useState } from "react";
import { asIssue, post } from "@/lib/api";
import { perGraph, Publisher, type Selection, type ShareState } from "@/lib/selection";

/** `rename`: a graph open here was renamed (plan 215); the publisher moves to its live alias. */
export type Share = ShareState & { setSharing: (on: boolean) => void; rename: (from: string, to: string) => void };

/**
 * What the publisher should hold now. While the view is between sets (the selection was reset to no graph), no graph
 * is open as far as publishing goes: every graph that left is cleared at once, even if the next set never loads
 * (review B3), and nothing is sent to a graph joining. A selection made in other graphs than those open is
 * not published (null: no change). Otherwise each open graph gets its own ids.
 */
export function shareTarget(selection: Selection, aliases: readonly string[]): ReadonlyMap<string, readonly string[]> | null {
  if (selection.graph === null) return EMPTY;
  if (selection.graph !== aliases.join(",")) return null;
  return perGraph(aliases, selection.ids);
}
const EMPTY: ReadonlyMap<string, readonly string[]> = new Map();

/** `aliases`: the graphs open now; the selection goes to each graph as its own ids (plan 133). */
export function useSelectionShare(selection: Selection, aliases: readonly string[]): Share {
  const [state, setState] = useState<ShareState>({ sharing: true, failures: [] });
  const [publisher] = useState(
    () =>
      new Publisher({
        post: async (graph, nodes) => void (await post(`/api/graphs/${encodeURIComponent(graph)}/selection`, { nodes })),
        changed: setState,
        setTimer: (fn, ms) => setTimeout(fn, ms),
        clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
        asIssue,
      }),
  );
  const target = useMemo(() => shareTarget(selection, aliases), [selection, aliases]);
  useEffect(() => {
    if (!target) return;
    publisher.update(target);
    return () => publisher.cancel();
  }, [publisher, target]);
  useEffect(() => () => publisher.stop(), [publisher]);
  const setSharing = useCallback((on: boolean) => publisher.setSharing(on), [publisher]);
  const rename = useCallback((from: string, to: string) => publisher.rename(from, to), [publisher]);
  return { ...state, setSharing, rename };
}

// The node panel's similar_to state (review R199-3): keyed by graph alias, node id and request, so a reply for
// another graph, another node or an older request never shows.
import { describe, expect, it } from "vitest";
import { similarAsked, similarFor, similarReplied, type SimilarState } from "@/lib/similar";
import type { SearchResult } from "@/lib/types";

const ID = "01ZZZZZZZZZZZZZZZZZZZZZZZA";
const list = (title: string): SearchResult => ({
  mode: "similar",
  query: "",
  hits: [{ id: "01ZZZZZZZZZZZZZZZZZZZZZZZB", type: "topic", title, status: "open", score: 0.8, legs: null, exact: null }],
  warnings: [],
  note: null,
  foreign: 0,
});

describe("similar_to state", () => {
  it("keeps the same node id in two graphs apart", () => {
    let s: SimilarState = similarAsked("alpha", ID, 1);
    s = similarReplied(s, 1, { result: list("from alpha") });
    expect(similarFor(s, "alpha", ID)?.result?.hits[0].title).toBe("from alpha");
    expect(similarFor(s, "beta", ID)).toBeNull();
    s = similarReplied(similarAsked("beta", ID, 2), 2, { result: list("from beta") });
    expect(similarFor(s, "beta", ID)?.result?.hits[0].title).toBe("from beta");
    expect(similarFor(s, "alpha", ID)).toBeNull();
  });

  it("drops a delayed reply for graph A after the view moved to graph B", () => {
    let s: SimilarState = similarAsked("alpha", ID, 1);
    s = null; // the graph switched to beta: the app clears the state
    s = similarAsked("beta", ID, 2);
    s = similarReplied(s, 1, { result: list("late alpha") });
    expect(s?.loading).toBe(true);
    expect(s?.result).toBeNull();
    s = similarReplied(s, 2, { result: list("beta") });
    expect(similarFor(s, "beta", ID)?.result?.hits[0].title).toBe("beta");
  });

  it("drops a reply after the state was cleared, and one for an older ask of the same node", () => {
    expect(similarReplied(null, 1, { result: list("late") })).toBeNull();
    const again = similarAsked("alpha", ID, 3);
    expect(similarReplied(again, 2, { error: { code: "x", message: "late" } })).toBe(again);
  });
});

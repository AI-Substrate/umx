---
id: 200
title: Read a search reply's hints for what it left out before searching again
route: explore
status: current
source: PRD 002 search-13, partly (change request 001, ruling r-03: hints throughout); secured_graphs_skipped and the search similarity notes are hints in every verb that raises them
reqs: [req-139, req-30]
supersedes: none
---
## Do
After `umx search`, read the reply's `hints` before you search again or stop. Each says what the reply did not show and gives a command:
- `search_results_cut`: `--limit` cut the list ("5 of the 12 matches found are shown"). Run its next with your words in place of `<words>` to see the rest (`umx search <words> --limit 12`; it keeps your `--graph`, `--scope` or `--all`, `--within` and `--along`), or search with more exact words.
- `similarity_incomplete`: some nodes have no vector yet, so only their words can find them. `umx embeddings status` shows how many are pending; search again once they land if meaning matters.
- `similarity_unavailable` and `similarity_no_embedder`: a graph was searched by words only, and the hint says why (no embedder, no vectors yet, a setting that admits no instance here, the query embed took too long). The results are right for words.
- `secured_graphs_skipped`: secured graphs were left out and only counted. Name one with `--graph` only when the user asked for it.

A reply lists at most 8 hints, then `hints_truncated`. `search_results_cut` and `secured_graphs_skipped` are always among them, even when many graphs each gave a `similarity_unavailable`.

## Don't
Don't read the absence of a hint as "nothing more exists" for every case: a search that nothing cut and that read every node by meaning has none of these hints, and that is the only time the list is complete. Don't treat a hint as an error: the reply's status stays ok, and the rows, their order and their scores are the same with or without hints. Don't run a secured graph's search, or change a graph's embedder setting, on your own: those are the user's calls. Don't read `search_results_cut`'s count as every node holding the words: every exact match is counted, but the word and meaning legs each give at most their best 100.

## Example
```bash
umx search walrus --limit 5 --graph demo
# hints: search_results_cut "5 of the 12 matches found are shown."  next: umx search <words> --limit 12 --graph demo
umx search walrus --limit 12 --graph demo
umx search approval                     # hint secured_graphs_skipped: 1 secured graphs were not searched ...
```

## Why
An agent reading a search reply could not tell what the reply did not say: that the results were cut by `--limit`, that some nodes had no vector yet so meaning missed them, that a graph was searched by words only and why, or that secured graphs were left out. Each silent condition is a place an agent stops too early or searches again blindly. These were warnings or nothing; now each is a hint with the command that shows the rest, and the reply's status stays ok because nothing the caller asked for was left undone.

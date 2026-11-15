---
id: 160
title: a search on a graph no embedder instance may serve answers by words and says so with similarity_no_embedder
route: explore
status: current
source: ws004 s32 (plan 145); project lead user report 2026-11-12 (testers, shapes only)
reqs: [req-53, req-46, req-91]
supersedes: none
---
## Do
Read `similarity_no_embedder` as: this graph's embedder setting (`local`, for a secured graph) admits none of this
machine's embedder instances, because every instance is hosted. Its query was never sent. It was searched by words
and exact matches only, so an empty result means no word matched, not that nothing is similar. Try other words or
word forms, or `umx node find <words>` for titles, and keep working.

Tell the user that meaning search is off for that graph and give both fixes: add a local embedder instance
(`umx docs get embeddings`), or, if the graph's owner chooses, allow hosted instances with the command the warning
prints, `umx graph set <graph> --embedder trusted`. Lowering a secured graph takes a person at their own terminal
(`umx graph confirm-policy`).

## Don't
Don't run `umx graph set <graph> --embedder trusted` yourself: whether a graph's text may go to a hosted instance is
the owner's call. Don't read the empty reply of a graph that warned `similarity_no_embedder` as "nothing about
this". Don't retry with `--include-named` or `--all` to get meaning: visibility never widens which instances a
graph may use.

## Example
```bash
umx search "vendor onboarding" --graph client_x
# rows: (none)
# warning: Similarity was not used for client_x: its embedder setting is local, which admits none of this
#   machine's embedder instances. Those results are word matches only. Add a local instance
#   (umx docs get embeddings), or its owner may allow hosted ones: umx graph set client_x --embedder trusted.
#   [similarity_no_embedder]
#   next: umx docs get embeddings
```

## Why
Before s32, a secured graph that recorded no model, on a machine with only a hosted instance and no
`embedder.active`, was dropped from the vector leg with no warning, so a search with no word match came back empty
and silent and readers took it as "nothing matches". With a hosted default the reply warned only
`embedder_not_local_for_secured`, which says nothing about search or the fix. Now every graph a search leaves
without vectors says why, once (req-53), and the query text of a `local` graph still never leaves the machine
(req-46, req-91).

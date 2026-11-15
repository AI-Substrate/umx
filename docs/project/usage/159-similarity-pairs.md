---
id: 159
title: umx similarity pairs lists each node's nearest nodes in the OTHER graphs, per call and never stored
route: explore
status: superseded
source: ws004 s31 (plan 134); embeddings-expert, architect, security and graph-storage-expert design round 2026-11-12
reqs: [req-136, req-65]
supersedes: none
superseded_by: 177
---
## Do
Use `umx similarity pairs --graph a,b[,c]` to find nodes in different graphs that mean the same thing (the graph view
draws these as shadow connections). Each row is one node, one of its top `--k` (default 3, at most 10) in another
graph, its `score` and its `rank`. Read `model` and `model_source`: only graphs whose vectors are in that model (graph.json's
recorded model, else the graph's embedder's) are compared; `similarity_model_mismatch` names the ones left out (pick another with `--model <model@dims>`); each `skipped_model` entry says
`another model` or `no vectors yet`, and a graph with no vectors yet never chooses the model. Read
`min.source`: the default cut is the model's similar floor (`calibrated` or `config`); `none` means nothing is cut
and the call warns `floor_uncalibrated`. Raise the bar with `--min 0.7000` (4 decimals). Page with `--limit` and
`--offset` while `truncated` is true. Treat each pair as a lead: read both nodes before adding an edge.

## Don't
Don't expect pairs inside one graph unless you pass `--same-graph`; use `umx neighbours <node> --relation
similar_to` for one graph's stored lists (same scores). Don't add `--all --include-named` unless the user asked
for secured graphs. Don't retry `similarity_pairs_too_large` as is: name fewer graphs. Don't compare scores across
models. Nothing is written, so there is nothing to commit or rebuild.

## Example
```bash
umx similarity pairs --graph finance,ops --k 3
# rows: [{"graph":"finance","id":"...","title":"Approval delay","other_graph":"ops","other_id":"...",
#         "other_title":"Slow sign-off","score":0.8731,"rank":1}, ...]
# model "text-embedding-3-large@256", model_source "named-first", min {"value":0.6133,"source":"calibrated"}
```

## Why
similar_to stays inside one graph by design, so links by meaning between graphs had no home. Computing them per
call from the vectors already in memory keeps graph files unchanged and lets any loaded set be compared, at a cost
bounded before anything is copied (2,000,000,000 pairs, 512 MiB) and never holding up a write.

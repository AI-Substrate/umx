---
id: 177
title: umx similarity pairs gives every row in one call (--limit up to 250000); page only when a call is too big
route: explore
status: current
source: ws004 s34a (F194 part a, plan 172); embeddings-expert, graph-storage-expert, security and architect design round 2026-11-13; supersedes the paging advice of 159
reqs: [req-136, req-65]
supersedes: 159
---
## Do
Use `umx similarity pairs --graph a,b[,c]` to find nodes in different graphs that mean the same thing (the graph view
draws these as shadow connections). Each row is one node, one of its top `--k` (default 3, at most 10) in another
graph, its `score` and its `rank`. Read `model` and `model_source`: only graphs whose vectors are in that model (graph.json's
recorded model, else the graph's embedder's) are compared; `similarity_model_mismatch` names the ones left out (pick another with `--model <model@dims>`); each `skipped_model` entry says
`another model` or `no vectors yet`, and a graph with no vectors yet never chooses the model. Read
`min.source`: the default cut is the model's similar floor (`calibrated` or `config`); `none` means nothing is cut
and the call warns `floor_uncalibrated`. Raise the bar with `--min 0.7000` (4 decimals).
To read every row, ask once with a `--limit` of at least the nodes compared times `--k` (at most 250000; the
default is 1000): each call compares every pair again, so a page costs as much as all the rows. Treat each pair as a
lead: read both nodes before adding an edge.

## Don't
Don't page through every row with `--limit` and `--offset`: over 12 graphs of 2k nodes at `--k 10`, 24 pages of
10000 took about 15 s through the CLI, one call of 240000 rows about 1.2 s. Don't expect pairs inside one graph unless you pass `--same-graph`; use `umx neighbours <node> --relation
similar_to` for one graph's stored lists (same scores). Don't add `--all --include-named` unless the user asked
for secured graphs. Don't retry `similarity_pairs_too_large` as is: name fewer graphs. Don't compare scores across
models. Nothing is written, so there is nothing to commit or rebuild.

## Example
```bash
umx similarity pairs --graph finance,ops --k 10 --limit 250000
# rows: [{"graph":"finance","id":"...","title":"Approval delay","other_graph":"ops","other_id":"...",
#         "other_title":"Slow sign-off","score":0.8731,"rank":1}, ...]
# total equals the rows returned and truncated is false
# model "text-embedding-3-large@256", model_source "named-first", min {"value":0.6133,"source":"calibrated"}
```

## Why
similar_to stays inside one graph by design, so links by meaning between graphs had no home. Computing them per
call from the vectors already in memory keeps graph files unchanged and lets any loaded set be compared, at a cost
bounded before anything is copied (2,000,000,000 pairs, 512 MiB) and never holding up a write. Because each call
computes everything, the limit was raised to 250000 rows (24k nodes at k 10) and a client reads a daemon reply of up
to 256 MiB (requests stay at 16 MiB), so one call carries every row (F194).

---
id: 182
title: Read similarity pairs as ids with --rows ids, and re-read them only when embeddings status' vectors_digest moves
route: explore
status: current
source: ws004 s34b (F194 part b, plan 178); embeddings-expert, graph-storage-expert, architect and security design round 2026-11-13
reqs: [req-136, req-65]
supersedes: none
---
## Do
When you only need which nodes pair up and how strongly, ask `umx similarity pairs --graph a,b --rows ids`. Each row
is then `graph`, `id`, `other_graph`, `other_id` and `score`: the same rows, in the same order and with the same
bytes for those fields as the full rows, at about 125 bytes a row instead of about 390. It takes the same graphs and
scope rules as full rows. Use `umx node get <id>` on the pairs you act on.
If you keep pairs (or anything else made from a graph's vectors) between calls, read `umx embeddings status` first:
each open graph has a `vectors_digest`. Read the pairs again only when a graph's digest changed. It changes when a
vector is added, removed, made in another model, or replaced after a text edit once the new vector lands, even when
`nodes` and `vectors_done` stay the same; it stays the same across writes that change no vector and across a daemon
restart.

## Don't
Don't use `--rows ids` when you need titles, types, addresses, policies or `rank`: ask for the full rows. Don't treat
`vectors_digest` as proof that two graphs' vectors match: it is made from the model key and each vector's node id and
text hash, not from the numbers, so a provider changing the model behind the same key, or the same text giving
slightly different numbers, does not change it. Don't expect a digest for a graph with no model yet or one that is
not open: it is `null` there (`embeddings status` takes no `--graph`). Don't poll status in a tight loop; a repeated call
is cheap, but the first after a change hashes the graph's vector lines.

## Example
```bash
umx embeddings status
# graphs: [{"graph":"finance","nodes":2000,"vectors_done":2000,"vectors_digest":"5b1f...e2", ...}, ...]
umx similarity pairs --graph finance,ops --k 10 --limit 250000 --rows ids
# columns: ["graph","id","other_graph","other_id","score"]
# rows: [{"graph":"finance","id":"...","other_graph":"ops","other_id":"...","score":0.8731}, ...]
```

## Why
The graph view reads every pair over 12 graphs (240k rows) and caches them. Full rows are about 88 MiB where the five
fields it reads came to 28 MiB in `--rows ids`, and node and vector counts cannot show a text edit that was re-embedded. A lean row
form and a per-graph digest of the stored vectors (the same hash `similar/<model@dims>/_meta.tsv` records, under the
model key) let a client read less and only when something changed (F194).

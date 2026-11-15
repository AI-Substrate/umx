---
id: 124
title: Keep writing while a graph embeds; commit similar/ once its backlog has drained
route: build
status: current
source: ws004 s25a; graph-storage-expert and embeddings-expert, 2026-11-11
reqs: [req-35, req-65]
supersedes: none
---
## Do
Keep writing while vectors are still being made, even a whole graph's worth after a restart or a model change: `node add`, `node edit`, `node note` and `umx batch` return in their usual time, because the daemon writes vectors only between client writes, one shard file at a time.

`similar_to` answers are current throughout: the lists in memory move with every vector. Their files under `similar/` lag. They are written once the daemon's whole backlog is quiet (every graph, not just yours), and on `daemon stop`, `daemon restart` and `graph unload`. While a long backlog keeps going, they are written a file at a time from about a minute on, and `_meta.tsv` is removed until the last of them is written.

Committing `similar/` mid-drain is safe: files that lag their vectors (or have no `_meta.tsv`) are never served; whoever loads them gets `similar_to_incomplete` and a rebuild. To commit them complete while vectors still arrive, run `umx graph unload <alias>` first (it writes everything the graph owes, then `umx graph load` it again). `umx similarity rebuild` also writes every file, but may refuse with `vectors_kept_changing` while vectors keep arriving.

After a crash mid-drain, the next load warns `similar_to_incomplete` and similar_to returns empty lists until the background rebuild lands (or run `umx similarity rebuild --graph <alias>`).

## Don't
Don't pause or slow your writes to "let embedding catch up": nothing is gained. Don't take `vectors_pending: 0` on one graph to mean its `similar/` files are written: another graph's backlog can hold them back. Don't read an empty similar_to list under `similar_to_incomplete` as "nothing is similar". Don't hand-fix a `similar/` folder committed mid-drain: a load that finds it behind (or without `_meta.tsv`) warns `similar_to_incomplete` and rebuilds it (usage 081).

## Example
```bash
umx daemon restart                          # every node of the graph is pending again
umx batch --file ops.jsonl                  # returns in its normal time while vectors arrive
umx graph unload demo                       # before committing similar/ mid-drain: writes what it owes
umx graph load /work/demo-graph             # and carry on
```

## Why
Each batch of vectors used to rewrite every similar_to file it touched while holding the graph's write lock, so a 20-op batch waited 30 to 60 s behind a big backlog. Now client writes go first, each background write holds the lock for one file, and the list files are written once for the whole drain. A crash mid-drain loses nothing: `_meta.tsv` is gone or describes other vectors, so the next load rebuilds the lists.

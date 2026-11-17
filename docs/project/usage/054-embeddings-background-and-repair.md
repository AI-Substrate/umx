---
id: 054
title: Vectors come after the edit; repair a graph's shards after a merge
route: build
status: planned (shards, change detection and repair ship in ws004 s07a; they run once an embedder is configured, ws004 s05)
source: ws004 s07a (plan 023); workshop 008 sections 2.5 and 4.1; graph-storage-expert acceptance for s07
reqs: [req-34, req-35, req-36]
supersedes: none
---
## Do
Write nodes as usual and move on: `node add`, `node edit` and `node note` return as soon as the node is saved. Its vectors (one for the title, the note and each content field, one for the whole node) are made afterwards in the background and land in `embeddings/<model@dims>/<shard>.tsv` in the graph folder. Commit those files with the graph.

After a git merge or pull, run `umx embeddings repair --graph <alias>`. It keeps the first of two equal lines, drops leftover ones, and queues any node whose vector is missing, stale or conflicting.

A secured graph (`"policy": "secured"` in graph.json) is embedded only by a local embedder, into a private cache under `$UMX_HOME/vectors`, never into the graph folder. `embedder_not_local_for_secured` means the configured embedder would send text off the machine, so nothing of that graph is embedded; its word search still works.

Read `vector_orphan` as "a line for a node this graph no longer has": the line is kept, nothing is lost. `vector_line_kept` means a line umx cannot read (often a merge marker); repair leaves it, a person removes it.

`vector_wrong_width` (a vector of the wrong width, never used; the node embeds again) and `vectors_unreadable` (the shards could not be read; nothing was queued) are warnings on load or repair: run repair after fixing what the message names.

## Don't
Don't wait for vectors after an edit, and don't re-save a node to "refresh" its vector: an unchanged text is never embedded again, so it does nothing. Don't hand-edit shard lines or delete shards to force a re-embed; run repair. Don't put secrets in node text expecting it to stay local: a hosted embedder sends text off the machine (secured graphs, ws004 s08 and s12, keep vectors local).

## Example
```bash
umx node edit <id> --set impact="Teams wait a day"   # returns now; impact and whole-node vectors follow
git pull                                             # both branches embedded the same node
umx embeddings repair --graph demo                   # queued 1, tidied 1, shards_written 1
```

## Why
Embedding can take seconds (or wait out a provider's rate limit), so it never blocks a write (req-35). Vectors are keyed by a hash of their text, so only changed fields re-embed and a cloned graph loads with zero embeds (req-36). Lines are sorted by node and field and sharded by node, so most merges are clean and the rest are one repair away.

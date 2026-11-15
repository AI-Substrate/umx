---
id: 081
title: Read similar_to edges by naming the relation; rebuild them, never edit them
route: explore
status: current
source: ws004 s09, plan 037; workshop 008 section 9; embeddings-expert D28 and 2026-11-10; cypher-expert 2026-11-10
reqs: [req-65]
supersedes: none
---
## Do
To see what umx found similar to a node, name the relation: `umx neighbours <node> --relation similar_to`. `out` rows are the node's own top k by whole-node vector, best first; `in` rows are the nodes whose top k holds it. Each row has a `score` (a cosine, 4 decimal places) and `derived: true`; its `edge` is null because these edges are not records. The result also says the `model`, `k` and `floor` (`floor_calibrated` is false for every model today).

Walk or find a path along them with `--relation similar_to` too, at most 2 steps (`umx walk <node> --relation similar_to --depth 2`).

Commit the `similar/` folder with the graph. If a load or a result warns `similar_to_incomplete`, the stored lists are being rebuilt (after a merge, or a settings change) or a node has no vector yet; to rebuild now, run `umx similarity rebuild --graph <alias>`.

## Don't
Don't expect similar_to in an unnamed step: `umx neighbours <node>` and `umx walk` without `--relation similar_to` never follow them, and neither do relation lists or counts. Don't treat a similar_to edge as a fact: it is a ranking; if two nodes really are related, add a real edge. Don't edit or merge-fix `similar/` files by hand: rebuild them. Don't run `umx edge get` on a similar_to row: it has no id. Don't walk similar_to more than 2 steps; it is refused.

## Example
```bash
umx neighbours 01JC8XA4H7 --relation similar_to --direction out
umx similarity rebuild --graph launch
```

## Why
umx keeps similar_to lists for every graph with vectors, stored in the graph folder so a clone reads them without computing; the scores are exact and the same on every machine. Naming the relation keeps rankings out of ordinary graph walks.

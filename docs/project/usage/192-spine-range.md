---
id: 192
title: Read along a spine with --along, --after and --before instead of asking "as at"
route: query
status: current
source: D71 step 12, plan 206 (time review R3: a range filter on a spine in place of "as at"; design agreed by pm-001, the architect, cypher-expert, graph-addressing and graph-storage)
reqs: []
supersedes: none
---
## Do
To ask what came up before, after or between points in a graph's order, add `--along <spine>` with `--after <end>` and `--before <end>` to `umx node list`, `umx node find`, `umx edge list`, `umx search` or `umx query`. Name a declared spine (`umx graph spine show <alias>` lists them; its ends are anchors: an id, a unique prefix or `umx:<this graph>/<id>`), or a built-in: `recorded`, `status` or `date_field:<f>` (its ends are dates in any form umx reads). Both ends are inclusive, either may be left out, and the range is applied before paging (on `umx edge list`, an edge into or out of another graph is never in range: it is counted as not placed, and `--unplaced` cannot list it), so `total` and `--offset` count rows in range. Read the reply's `along` object (spine, `source`, clock, ends, `not_placed`, `by_rule`, `coarse`, `spines`) and its hints: `along_not_placed` and `along_by_rule` give how many rows were left out because the spine does not place them or places them only by rule, with the command that lists them (`umx graph spine show <alias> --unplaced <spine>` or `--by-rule <spine>`). Link those nodes to their anchor and read again. For node list, node find and edge list the counts are of the rows the other filters kept; for search and query, of the graph's nodes.

## Don't
Don't use `--along` across graphs: with `--graph a,b`, `--scope`, `--all` or `--provenance` it is refused. Don't pass a node that is not one of the spine's anchors as an end (`along_end_invalid`, whose candidates are the first anchors in order), nor an address of another graph, the bare graph address or a fragment. Don't read a node left out by rule as "not in the range": it has no place by data at all. Don't name a declared spine `recorded` or `status`: those are the built-ins (`spine_name_reserved`); one declared before this is still read by `--along` under that name, and `umx check` warns `spine_name_reserved_stored`. Don't expect a query to reach past the range: no path passes through a node outside it, and an `OPTIONAL MATCH` to one gives null.

## Example
```bash
umx graph spine show demo
umx node list --along day --after 01JC8XA4H7 --before 01JC8XB2K9
umx edge list --along day --before 01JC8XB2K9
umx search vendor delay --graph demo --along day --after 01JC8XA4H7
umx query 'MATCH (d:decision) RETURN d.title AS decision' --graph demo --along day --before 01JC8XB2K9
umx node list --along date_field:due_on --after 2026-12-01 --before 2026-12-31
umx graph spine show demo --unplaced day
```

## Why
"As at" was dropped (R3): the real questions (the playhead, what the room had agreed before lunch) are a range on a spine, and "what held true on a date" is two plain date filters. One placement rule decides where every node sits, so the CLI, the view and every verb give the same range, and every reply says what it left out (B4a), never silently.

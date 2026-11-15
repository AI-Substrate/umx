---
id: 162
title: Fix a wrong cross-graph edge from the graph that stores it, with edge edit or edge delete
route: build
status: current
source: F122, plan 144
reqs: [req-13, req-88, req-89]
supersedes: none
---
## Do

An edge with an end in another graph is fixed like any edge, from the graph that stores it:

- `umx edge edit <edge> --relation <r> --graph <storing graph>` to retype it,
- `umx edge edit <edge> --reverse --graph <storing graph>` to swap its ends,
- `umx edge delete <edge> --graph <storing graph>` to remove it.

Find the edge with `umx edge list --from <node> --graph <storing graph>`, and name it by its full id or its
address in that graph. The other graph does not need to be loaded or named. A batch takes `edge edit` and
`edge delete` lines for cross-graph edges too.

A retype to a symmetric relation (`same_as`, `conflicts_with` and the like) stores the edge with the lower
(graph id, node id) end as `from`, so the far node can become `from`: the reply's `changed` then holds `direction`,
and the hint `edge_direction_normalised` names the relation and the ends as stored (F182).

## Don't

Don't expect the reply to tell you anything about the far node: it shows the stored address only, never the far
node's title, whether or not that node exists or its graph is loaded or secured. Don't try to point an edge at a
different node with `edge edit`: an edit never moves an end. Delete the edge and add a new one. Don't edit
edges.jsonl or graph.json by hand.

## Example

    umx edge list --from 01JC8XA4H7 --graph alpha
    umx edge edit umx:alpha/01JC8XE5RT2W9V8B3N4M5K6J7H --relation relates_to
    umx edge delete umx:alpha/01JC8XE5RT2W9V8B3N4M5K6J7H

## Why

Agents repairing wrong links into another graph had no verb: edit and
delete refused any edge with an end in another graph. The storing graph owns the edge, so it can change it
without touching the far graph, and the reply stays the same whatever the far graph's state, so it reveals
nothing about it.

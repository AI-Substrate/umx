---
id: 098
title: Select graph lists or reuse a saved scope
route: multi-graph
status: superseded
source: ws005 s07a plan 066, graph-addressing-expert and architect rulings 2026-11-11
reqs: [req-66]
supersedes: none
superseded_by: 100
---
## Do

Use `--graph a,b` for a list of loaded aliases on node list, edge list, query or search.
Use `umx scope save both --graph a,b` to retain that selection, then `--scope both` to reuse it.
The scope records graph ids, with alias and folder hints, in `$UMX_HOME/scopes.toml`.
Renaming a loaded alias does not change the selection. Show a scope to see which members are loaded or secured.
Use `--replace` to replace an existing saved name.

## Don't

Do not use `--set` or `--scope a,b` to select graphs, combine `--graph` with `--scope`, or add `--all`.
Do not expect scopes on writes, node find, walk, neighbours or path yet. These still take one graph.
Do not treat a scope as a copy selection: saved graph ids resolve to primary copies.
Do not ignore `scope_graph_not_loaded`: the whole read was refused, not returned short.

## Example

    umx node list --graph design,delivery
    umx scope save both --graph design,delivery
    umx node list --scope both
    umx edge list --scope both --type blocks
    umx query 'MATCH (n) RETURN n' --scope both
    umx search approval --scope both
    umx scope show both
    umx scope list
    umx scope delete both

## Why

Scoped lists label every row with its graph and address; limit/offset apply globally.
The one-graph default is unchanged. Unnamed query/search keep secured graphs out and one primary per graph id.
A saved scope counts as explicitly naming its members, including secured graphs, so saving one warns
`scope_includes_secured`. Reading explicit copies with `--graph` keeps d-21's copy labels and warning.

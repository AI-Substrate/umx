---
id: 163
title: In a view with several graphs open, umx ui highlight and umx ui clear act only within the graph they name
route: explore
status: current
source: workstream 009 s10 M1a, plan 133 (project manager, 2026-11-12)
reqs: []
supersedes: none
---
## Do

The view can show several graphs on one canvas (a person picks them, or a launch link names `g=a,b,c`). Each
`umx ui` command still belongs to one graph, chosen as before: the address's graph, else `--graph`, else
`UMX_GRAPH`, else the one open graph.

- `umx ui highlight <node>...` replaces your highlight in THAT graph only. Highlights you sent to the other open
  graphs stay. To mark nodes in two graphs, send one highlight per graph (or use addresses).
- `umx ui clear --graph <alias>` takes off what agents set in that graph only: its highlight, a focus on one of its
  nodes, and a filter sent for it. Clear each graph you pointed at, or every one when you are done.
- `umx ui filter` sets the chips for the whole canvas (types, statuses and relations are shared by the open graphs).
- `umx ui focus <node>` focuses that node; focus then walks across edges into the other open graphs.
- `umx ui selection --graph <alias>` lists what the person selected in that graph; a selection that spans graphs
  is published to each graph separately.

## Don't

- Don't expect a highlight in one graph to clear the highlight you sent to another.
- Don't expect `umx ui clear` in one graph to remove a filter you sent for another graph.
- Don't try to open or name a secured graph in the person's view: only the person opens one. It is counted in the
  view, never named.

## Example

    umx ui highlight umx:alpha/01JC8XA4H7 --note "alpha's side"
    umx ui highlight umx:beta/01JC8XB2QK --note "beta's side"
    umx ui clear --graph beta

## Why

A person looking at several graphs at once wants each agent command to stay where the agent pointed. Before
plan 133 the view showed one graph, so "replace" and "clear" could only mean that graph.

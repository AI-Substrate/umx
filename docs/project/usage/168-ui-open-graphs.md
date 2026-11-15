---
id: 168
title: Open graphs in the person's view with umx ui open, and several at launch with umx ui --graph a,b,c
route: explore
status: current
source: workstream 009 s10 M1b1, plan 154 (the project manager, 2026-11-12)
reqs: []
supersedes: none
---
## Do

- To show a person more graphs, run `umx ui open --graph beta,gamma --note "<why>"`. The graphs are added to what is
  open; nothing the person sees moves unless they follow you and are idle. Name 1 to 12 graphs, each once.
- To show only some graphs, add `--only`. The view always asks the person first ("closes alpha"); nothing changes
  until they choose Open.
- Read the reply: `sent` lists the graphs sent to the view (it opens at most 12 at once and counts any past that, so
  `sent` is not what it shows), `refused` lists each graph left out with its `kind` (`secured`, `unknown`,
  `unloaded`). On `ui_open_left_out` (some left out) or `ui_open_refused` (none sent: one error per graph, in the
  order you named them), run each one's next: `umx graph list --all` for an unknown graph, the `umx graph load` it
  names for one unloaded on purpose.
- To launch the view with several graphs, run `umx ui --graph alpha,beta,gamma`. The JSON reply has `graphs` (and
  `graph`, the first). A `stale_files` warning with it means a graph's files changed on disk; run its next.

## Don't

- Don't try to open a secured graph with `umx ui open`: it is always refused, even when the person shows secured
  graphs. Tell the person; they open it by clicking it, or with `umx ui --graph vault`.
- Don't use `--only` to tidy the view on your own: it asks the person every time.
- Don't read `sent` as on screen: the view may have left a graph out at its 12-graph cap.
- Don't name more than 12 graphs, or one graph twice (`usage_invalid`).

## Example

    umx ui open --graph beta,gamma --note "the other side of this decision"
    umx ui open --graph beta --only --note "just the decision log"
    umx ui --graph alpha,beta,gamma

## Why

Before plan 154 an agent could point inside the graphs a person had open, but could not bring another graph into
the view, and `umx ui --graph` launched with one graph. Agents can now add graphs without moving what the person is
looking at, while secured graphs stay the person's to open.

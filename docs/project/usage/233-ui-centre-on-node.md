---
id: 233
title: Centre a person's graph view on a node with umx ui focus --centre, and put the layout back with umx ui clear
route: explore
status: current
source: PRD 005 centre on this node (centre-11, centre-13), 2026-11-27
reqs: []
supersedes: none
---
## Do

When a person has the graph view open (`umx ui`) and the question is "what is around this node", centre the view on
it:

- `umx ui focus <node> --centre` focuses the node, as `umx ui focus` does, and lays the view out in rings of hops
  around it: the node in the middle, its neighbours on ring 1, theirs on ring 2, across every open graph. Nodes no
  shown link reaches sit on an outer shelf. It is still a focus: rings past `--hops` (1 to 3, default 1) are dimmed,
  so pass `--hops 3` to keep three rings lit. Add `--note` as for any focus.
- `umx ui clear` in that graph puts the person's layout back. Only a centring an agent made is undone; the person's
  own "Centre on this" stays.
- Read `umx docs get ui` ("Centre on a node") for what the person sees: rings never overlap, the camera fits, a spine
  is paused while centred, and with entities merged an entity is centred on with all its members' links.

## Don't

- Don't centre to point at a node in passing: `umx ui focus` or `umx ui highlight` keeps the person's layout.
  Centring moves every node.
- Don't send `--centre` again and again to walk the graph; one centring per question, then let the person explore.
- Don't expect it to change the graph or survive a reload: it is view only.

## Example

    umx ui focus umx:acme/01JC8XA4H7 --centre --note "everything this decision touches"
    umx ui clear --graph acme

## Why

Jordan asked for a radial view around one node, by hops, that a person can open from the node panel and an agent can
open the same way (PRD 005). It is a presentation command like the other `umx ui` commands: it lives in the daemon's
memory, never in a graph, and the view applies it only when it is fresh, so an old command never re-arranges a view
that opens later.

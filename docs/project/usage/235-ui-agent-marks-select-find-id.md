---
id: 235
title: Select a node for the person with ui focus --select; the person clears agent marks and finds nodes by id
route: explore
status: current
source: umx fixes batch 1, 2026-11-27 (project lead: agents could not open a node's panel; agent marks came back on reload)
reqs: []
supersedes: none
---
## Do
Use `umx ui focus <node> --select` when the person should see the node's panel, not only where it is.
Tell the person they can take your marks off with "Clear agent marks" in the agent strip, or run `umx ui clear`.
Give the person a node's id or `umx:<alias>/<id>` address to paste into Find: it jumps to the node.
In Declared hubs with no hubs declared, suggest Degree or Similarity hubs, or declare hubs (`umx docs get hubs`).

## Don't
Do not send focus --select to take the person's panel while they work: the view selects only when it moves the camera.
Do not expect a cleared mark to come back: the clear is kept for every view.

## Example
    umx ui focus 01JC8XA4H7 --select --note "this one"
    umx ui clear --graph acme

## Why
The view selects the node with the camera move, so the person's own selection is never changed under them. A clear
from the strip is the same `umx ui clear` the daemon keeps, so a reload does not replay the marks.

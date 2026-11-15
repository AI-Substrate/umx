---
id: 139
title: A --scope read loads members that unloaded when idle
route: multi-graph
status: current
source: ws002 F99 (field report), project manager 2026-11-11; builds on F72 (usage 109)
reqs: [req-15, req-66]
supersedes: none
---
## Do
- Read a saved scope with `--scope <name>` as usual. A member that unloaded when idle (30 minutes by default), or
  did not come back after a daemon restart, is loaded for the read, with a `graph_loaded_on_demand` hint naming
  its folder. Read the warning; the command ran.
- On `scope_graph_not_loaded`, run its `next` (`umx graph load <folder>`): that member was unloaded on purpose.

## Don't
- Don't reload scope members by hand before every read.
- Don't expect a member you unloaded with `umx graph unload` to come back: unloaded stays unloaded, and the read
  loads no other member either.

## Example
```bash
umx scope save both --graph acme,acme_ops
# ... later, after idle unload:
umx node list --scope both      # loads acme and acme_ops, hints graph_loaded_on_demand for each
```

## Why
F72 made a named graph that unloaded when idle load on demand, but a `--scope` read still failed
`scope_graph_not_loaded` once its members idled out (F99). A saved scope names its members as explicitly as
`--graph`, so they load the same way, through the same checks; secured members load because the scope names them.

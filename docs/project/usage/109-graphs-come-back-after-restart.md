---
id: 109
title: Graphs come back after a daemon restart, and a named graph that unloaded loads on demand
route: start
status: current
source: ws002 F72 (field report batches 3 and 4), architect and security rulings 2026-11-11, plan 081
reqs: [req-56, req-15, req-03]
supersedes: none
---
## Do
- After `umx daemon restart`, `just dev`, a stop then start, or a service restart, carry on: every graph that was
  loaded comes back, in the same load order and under the same alias. Since 221 the daemon answers while they
  come back: a command naming its graph runs at once; one naming none waits until all are back.
- Name the graph you mean (`--graph <alias>` or an address). A named graph that unloaded when idle (30 minutes by
  default) or did not come back after a restart is loaded for that command, with a `graph_loaded_on_demand`
  hint. Read the hint; the command ran.
- If a graph is missing after a restart, run `umx doctor`. `doctor_graph_not_reloaded` gives the reason's code and
  the `umx graph load <folder> --as <alias>` to run once the cause is fixed. `umx daemon status` lists the same
  graphs under `registry.not_reloaded`.
- If you no longer want a graph that did not come back, run `umx graph unload <alias>`: the daemon forgets it.
- Treat `graph_not_loaded` as "you or another agent unloaded it" (its message says `it was unloaded`) or "this daemon
  never loaded it" (the message says when the daemon started). Run its `next`.

## Don't
- Don't reload graphs by hand after every restart, and don't keep a list of folders to reload: umx keeps one.
- Don't expect a command that names no graph to load one. Only a named graph loads on demand.
- Don't expect `umx graph unload` to be undone by a restart or by naming the graph: unloaded stays unloaded until
  you run `umx graph load`.
- Don't retry `graph_id_changed`: the folder now holds another graph. Load it under its own alias with the `next`.
- Don't edit `~/.umx/registry/graphs.toml`. If `registry_unreadable` says it cannot be used, nothing reloads and umx
  never rewrites it; tell the human, who can fix or remove it.

## Example
```bash
umx graph load ~/graphs/acme
umx daemon restart
umx node list --graph acme          # acme is back
# ... 30 minutes later, after idle unload:
umx node find "approval" --graph acme   # loads acme again, hints graph_loaded_on_demand
umx doctor                          # names any graph that did not come back
```

## Why
Field use found every graph silently gone after a restart or upgrade, and again after the 30-minute idle unload;
the next command failed `graph_not_loaded` with no hint. The daemon now keeps this machine's graph registry
(req-56's first slice) and reloads from it at start and on demand (req-15), through the same checks as
`umx graph load`, so a swapped folder or a lowered policy is caught exactly as on a manual load.

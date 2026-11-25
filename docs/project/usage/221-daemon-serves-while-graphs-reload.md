---
id: 221
title: The daemon answers at once after a start and reloads remembered graphs behind it
route: start
status: current
source: field report 2026-11-25 (a service daemon took 105 s to answer after a restart)
reqs: [req-56, req-15, req-86]
supersedes: none
---
## Do
- After `umx daemon restart`, a start, or the service coming back, carry on at once: the daemon answers within
  about a second and reloads the graphs it remembers one by one behind it, in load order.
- Name the graph you mean (`--graph <alias>`, `--graph a,b`, or `--scope`). A named graph still waiting is loaded
  next, ahead of the others, and the command runs.
- To see how far the reload is, run `umx daemon status`: `registry.reloading` is how many graphs still wait (0
  when all are back), and `registry.reloaded` lists those back so far.
- If `umx doctor` warns `doctor_service_throttled` or `doctor_service_path`, run `umx daemon install` again from
  the shell you normally use: the service is rewritten to run at normal priority with that shell's `PATH`, so it
  finds `az` and `azd`.

## Don't
- Don't read `umx graph list` right after a start as the full set: it shows the graphs back so far. Wait for
  `registry.reloading` to reach 0, or name the graph.
- Don't expect these to run before all graphs are back, even with `--graph`: `--all`, `--include-named`, `graph
  new`, `graph load`, the entity commands and `neighbours` (unless `--entities off`) cover more than the named
  graph, so they wait, as does any command naming no graph (`UMX_GRAPH` alone does not count).
- Don't treat a `daemon_busy` from one of those right after a start as a fault: it waited 60 s for the reload.
  Run it again.
- Don't edit the service file by hand to change its priority or `PATH`; `umx daemon install` writes both.

## Example
```bash
umx daemon restart
umx daemon status            # registry.reloading: 12
umx node list --graph crm    # runs now; crm loads first if it still waits
umx search "pricing"         # names no graph: waits until all are back
```

## Why
Reloading every remembered graph before answering made a cold or busy machine wait minutes for any command,
`daemon status` included. Serving first keeps the daemon responsive; loading a named graph first keeps named work
fast; and holding unnamed commands until all graphs are back means they never answer from a partial set.

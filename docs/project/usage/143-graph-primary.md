---
id: 143
title: Choose which copy of a graph is primary with umx graph primary
route: multi-graph
status: current
source: F98 (the project manager; graph-addressing-expert, architect and security approved the shape, 2026-11-12); d-21 and its F80 amendment
reqs: [req-69]
supersedes: none
---
## Do
When two or more copies of one graph are loaded (the same graph id under two aliases) and a command that names no
graph is refused with `copy_ambiguous`, run its next, `umx graph primary <alias>`, naming the copy that unnamed reads
should use. That copy is primary from then on, and still is after a daemon restart. Run it again any time to move the
primary to another copy that is loaded or remembered (one that unloaded after being idle loads again first); a copy
that already is primary, or the only copy, answers `unchanged: true`. If the reply warns `registry_update_not_saved`,
the choice holds until the daemon stops; run the same command again once the graph index can be written to save it.

## Don't
Don't make a secured copy primary unless you mean to: unnamed reads then skip that graph id entirely, and the reply
warns `primary_is_secured`. Read it by naming a copy with `--graph`, or make an open copy primary again. Don't expect
the reply to list the other copies; it names only the copy you gave. `umx graph list` shows what is loaded. Don't
name a copy you unloaded with `umx graph unload`: it is `graph_not_loaded` until you load it again.

## Example
```bash
umx graph load ~/work/design-b --as design_b
umx graph unload design            # the primary goes; with two copies left, none is primary
umx node list                      # copy_ambiguous, next: umx graph primary design_b
umx graph primary design_b
umx node list                      # reads design_b
```

## Why
Which copy unnamed reads use should be a choice, not an accident of load order, and it must survive a restart
(d-21, F80). Before this verb, a graph that lost its primary stayed without one until only one copy was left.

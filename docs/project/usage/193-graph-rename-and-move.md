---
id: 193
title: Rename a graph with umx graph rename and move it with umx graph move
route: multi-graph
status: current
source: ws006 S23, req-56 (plan 209; pm-001, graph-addressing-expert and architect rulings, 2026-11-13)
reqs: [req-56]
supersedes: none
---
## Do
To change a graph's alias, run `umx graph rename <alias> <new alias>`. graph.json keeps the old alias in `former_aliases`, so every `umx:<old>/<id>` address, evidence pointer and edge end keeps working: `umx resolve` answers `redirected`, and any command that names the old alias works with the hint `alias_redirected`. Write the new alias from then on. The registry, saved scopes and the Graphfile follow.

To move a graph's folder, run `umx graph move <alias> <new folder>` (inside the same git repo). The graph stays loaded through the move, vectors and queued work included. The reply lists the graph's `root:repo` pointers into the folder it left (`stale`, `evidence_root_stale`); re-cite each under the new folder. `umx check` keeps listing them until you do. Other graphs citing the old folder are not checked (`other_graphs_checked: false`).

## Don't
Don't edit `alias` in graph.json by hand: no `former_aliases` entry is written, so the old addresses break. Prefer `graph move` to moving the folder by hand. A hand move (`git mv`, then `umx graph load <new folder>`) heals: the load says `moved_from` and lists the stale `root:repo` pointers once (`stale_recorded: false`), but `umx check` won't keep listing them, so re-cite them from that reply. Other graphs citing the old folder aren't checked by either path. Don't move a graph into another git repo with `graph move` (it is refused with `graph_move_cross_repo`); use `git mv`, load it there, and re-cite its `root:repo` pointers. Don't move a graph whose folder name is not a repo path (`a:b`, `~archive`): graph.json cannot record it, so the move is refused; rename the folder with git first. Don't type the live alias when `graph confirm-policy <old>` asks for `<old>`: it is refused. Don't expect a renamed or moved graph to load in an older umx: it needs graph.json format 7.

## Example
```bash
umx graph rename launch launch_v2
umx resolve umx:launch/01JC8XA4        # status: redirected, graph launch_v2
umx graph move launch_v2 /work/repo/graphs/launch_v2
umx check --graph launch_v2            # evidence_root_stale rows, if any
```

## Why
Other graphs and repos cite a graph by its alias. A rename that keeps the old alias as a redirect, and a move that names the pointers it made stale, change a graph's name or place without silently breaking what points at it.

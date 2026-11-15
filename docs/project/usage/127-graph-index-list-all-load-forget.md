---
id: 127
title: Find any graph this machine has loaded with graph list --all, load it by alias, forget it
route: explore
status: current
source: ws002 s16 (decision D61, 2026-11-11) and F80 (graph-addressing-expert and architect, d-21 amendment); security, architect and graph-addressing-expert rulings on plan 098
reqs: [req-116, req-56]
supersedes: none
---
## Do
- To find a graph you or another agent used before, run `umx graph list --all`. It lists loaded graphs and every
  graph this machine loaded before, with `state` (`loaded`, `idle`, `not_reloaded`, `unloaded`), `folder`
  (`present`, `missing`, `outside_roots`), counts and model as of its last unload, `policy`, `repo`, and `reload`.
- To load one again, run its `reload` exactly, or `umx graph load <alias>` with the bare alias.
- When `graph load <alias>` says `alias_ambiguous`, pick the folder you mean from its candidates and run that
  candidate's command.
- To drop a graph you will not use again from the list: `umx graph unload <alias>`, then `umx graph forget <alias>`.
- With copies of one graph loaded (d-21), after the primary is unloaded name a copy with `--graph` until only one
  copy is left. This holds across a daemon restart too. `primary_restored` in `umx daemon status` means only one
  copy came back, so it is primary again.

## Don't
- Don't expect counts, model or repo for a secured graph that is not loaded: the row shows only its alias, id,
  path, state, folder, policy and reload.
- Don't run `graph forget` on a loaded graph (`graph_forget_loaded`); unload it first. `graph_forget_not_saved`
  means nothing was forgotten: run `umx doctor`, never edit `~/.umx/registry/graphs.toml` yourself.
- Don't treat `folder: missing` as a umx fault: the folder was renamed, moved or deleted. Load it from where it
  is now (`umx graph load <new folder> --as <alias>`), then forget the old row if it is still listed.
- Don't wait for a `umx graph primary` verb: none exists yet, so a graph with no primary copy gets one back only
  when one copy is left.

## Example
```bash
umx graph list --all
umx graph load acme                 # the folder the index remembers for acme
umx graph unload old_spike
umx graph forget old_spike          # no longer listed
```

## Why
Decision D61 wanted every graph loaded here findable after it unloads, with the command to load it again. The
index is F72's registry, which already kept unloaded rows. Security limited what an unloaded secured graph shows,
and graph-addressing-expert ruled that an alias naming two folders is never resolved by guessing.

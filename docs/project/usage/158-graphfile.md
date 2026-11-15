---
id: 158
title: Find a repository's graphs with umx graph scan and load them with --graphfile
route: explore
status: current
source: D66 and req-128 (decisions, 2026-11-12); ws002 s17, plan 137 (graph-addressing-expert, graph-storage-expert, security and architect rulings)
reqs: [req-128]
supersedes: none
---
## Do
On a repository you have not worked in, run `umx graph scan` before anything else. It lists every graph folder
in the repository against its `Graphfile` (the committed list of its graphs) and loads nothing. Then
`umx graph load --graphfile` loads what the Graphfile lists, or `--set <name>` one named set of it, and
`umx graph list --graphfile` shows what is loaded. Read each per-entry warning: `graphfile_entry_skipped` names
the code and the command to run (an alias already in use names the `--as` load), and the other graphs still load.

`umx graph new` adds the new graph to the repository's Graphfile by itself; the reply's `data.graphfile` names
the line and its `undo` (`umx graph scan <repo> --drop <alias>`). Pass `--no-graphfile` for a private graph or a
graph in a folder git ignores: umx does not read `.gitignore`.

## Don't
Don't run `umx graph scan --add` or `--drop` unless the user asked to change the Graphfile: it is committed and
shared. Don't add `--graphfile` to `graph new` for a secured graph unless the user wants everyone who clones the
repository to see that it exists. Don't hand-edit a Graphfile to silence `graphfile_invalid`; fix the line it
names. Don't treat a set as a scope: `--scope` reads, a set only loads.

## Example
```bash
umx graph scan                                   # rows: path, alias, id, state (listed, new, missing, moved...)
umx graph load --graphfile --set acme            # load one set
umx graph new /Users/me/work/repo/scratch --alias scratch --no-graphfile
```

## Why
Without a Graphfile an agent has no way to know which graphs a repository holds short of searching the disk.
The Graphfile is that list, committed beside the graphs, written byte-stable so git stays clean. It holds no
graph data, so it is never a central graph (D63).

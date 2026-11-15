---
id: 052
title: Cut-off writes, recovery_conflict and the .umx-tmp- files
route: doctor
status: current
source: workstream 003 s03 (2026-11-10); workshop 008 section 4.1; architect's write-set design
reqs: [req-01, req-04]
supersedes: none
---
## Do
A change to several files (a node with its parent edge, a new graph) is one write: all of it or none of it.
A `write_not_durable` warning on it means the whole change stands, as in note 026.

Leave `.umx-tmp-` files and `.umx-tmp-set.json` in a graph folder alone. `umx graph new` adds `.umx-tmp-*` to
the folder's `.gitignore` so git never sees them; keep that line if you edit the file, with nothing before it
(git reads leading spaces as part of the pattern, and the line then stops working).

On `recovery_conflict`, stop and tell the person. A crash cut off a write to several files, and the files the
error names changed since (a pull, a checkout, a hand edit). Every command on that folder fails, changing
nothing, until they choose:
- keep the files as they are now: delete `.umx-tmp-set.json` and the other `.umx-tmp-` files in the folder;
- or finish the cut-off write: put the named files back as they were before (for example `git checkout`) and
  run the command again.

For a loaded graph, `umx check` lists the files.

## Don't
Don't commit `.umx-tmp-` files, and don't delete them while the daemon is writing.

Don't settle a `recovery_conflict` yourself by deleting the marker or restoring files: which side wins is the
person's call.

## Example
```bash
umx graph load /Users/me/work/launch-graph
# error recovery_conflict: /Users/me/work/launch-graph holds a write that a crash cut off and umx will not
# finish, because edges.jsonl changed since. Nothing was changed.  next: umx docs get graph

umx check --graph launch     # for a loaded graph: one recovery_conflict row per file
```

## Why
A change to several files (a node with its parent edge, a new graph) is all or nothing, also across a crash.
umx writes a marker before the renames and finishes them on the next command; it will not write over a file
someone changed since, so neither side is lost without a person choosing.

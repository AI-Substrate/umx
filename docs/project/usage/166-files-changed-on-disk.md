---
id: 166
title: graph_files_changed and stale_files after a checkout, pull or hand edit
route: doctor
status: current
source: workstream 002 F144, plan 148 (2026-11-12); the graph-storage expert's storage ruling; architect's port ruling
reqs: [req-01]
supersedes: none
---
## Do
A loaded graph is the copy umx read from its files. When `graph.json`, `nodes.jsonl` or `edges.jsonl` changes
behind it (a `git checkout`, `git pull`, merge, hand edit, another tool, or only a `touch`):
- every write to it fails with `graph_files_changed`, naming the files; nothing is written and memory is as it was;
- every read still answers, from the old copy, with the warning `stale_files`.

Reload it to work on the files as they are now, with the `next` both codes carry (the two commands joined by `&&`):

    umx graph unload launch
    umx graph load ~/work/launch --as launch

Then check that what you meant to write is still needed against the new files, and write it again.

## Don't
Don't retry the refused write before reloading: it is refused every time, by design. umx never reloads on its own,
so the old copy can never be written over the checked-out files.

Don't switch branches under a loaded graph you are writing to. Unload it first, or reload after.

Don't treat an answer that carries `stale_files` as the files' truth.

## Example
```bash
umx graph load ~/work/launch               # on branch e1
git -C ~/work checkout main                # nodes.jsonl changes under the loaded graph
umx node list --graph launch               # e1's nodes, warning stale_files: nodes.jsonl
umx node add --graph launch --type finding --title "New"
# error graph_files_changed: The files of graph launch changed on disk since it was loaded: nodes.jsonl. Nothing
# was written.  next: umx graph unload launch && umx graph load /Users/me/work/launch --as launch
```

A touch alone (same bytes) is refused too: umx compares each file's identity, size and times, not its bytes, and
errs on the side of not writing.

## Why
Before F144 a write to a loaded graph rewrote its files from the copy in memory, whatever had happened to them on
disk. Load a graph on one branch, check out another under it, add a node: the checked-out `nodes.jsonl` was
silently replaced by the first branch's nodes plus the new one, and nothing said so. Refusing the write and warning
on reads turns that silent data loss into an error you can see and a reload you choose.

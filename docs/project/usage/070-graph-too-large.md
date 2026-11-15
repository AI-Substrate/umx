---
id: 070
title: graph_too_large means a graph file is over umx's size cap; nothing was read or written
route: doctor
status: current
source: workstream 003 follow-ups 2 (plan 036, 2026-11-10); security follow-ups, architect design
reqs: [req-01, req-04]
supersedes: none
---
## Do
When a command fails with `graph_too_large`, read which file it names and its size. The caps are: `graph.json`
1 MiB, `.gitignore` 64 KiB, a vector shard (`embeddings/<model@dims>/<xx>.tsv`) 64 MiB, `nodes.jsonl` and
`edges.jsonl` 4 GiB. Nothing was read past the cap and nothing was written.

- On `umx graph load` or `umx check`: the file on disk is over its cap. That is almost always a bad merge, a
  generated file or something copied into the folder by mistake. Stop and tell the human which file and size;
  `git log -p -- <file>` shows where it came from.
- On a write (`node add`, `node edit`, `ontology add ...`): the change would take the file over its cap. The
  graph is as it was. Don't retry the same change.

## Don't
Don't trim, split or rewrite the file yourself to get under the cap, and don't edit it while the graph is
loaded. Don't retry in a loop: the answer will not change.

## Example
```bash
umx graph load /Users/me/work/launch-graph
# error graph_too_large: graph.json is 1048581 bytes, over the 1048576 bytes umx holds for it. Nothing changed.
umx docs get graph        # § Size limits
```

## Why
Graph files arrive by git pull, so their size is not umx's to choose. umx checks the size on the open file before
reading it, and stops reading one byte past the cap, so a huge or growing file can't make the daemon allocate
without bound. It also refuses to write a file it would then refuse to read.

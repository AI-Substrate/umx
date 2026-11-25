---
id: 222
title: Check a graph before you commit and push it
route: build
status: current
source: prime packet check-sources (2026-11-25; Jordan: "base checks should warn", and agents check before they commit and push)
reqs: [req-67, req-77]
supersedes: none
---
## Do
Before you commit and push a graph folder, run `umx check --graph <alias>`. Besides the graph's own files (repeated ids, dangling edges, unknown keys, a missing `.gitignore` line), it reads every local file the graph cites again and warns `source_changed` (the file differs from when it was cited) and `source_missing` (it cannot be read now), listed in `data.sources.rows`. Fix what it reports. For `source_changed`, read what cites the file and check it still holds, then accept the file with `umx node evidence <node> refresh <pointer>` (or `--at <i>`) or `umx resource refresh <node>`. For `source_missing`, restore the file, or fix or remove the pointer. A refresh writes the graph's files, so commit after it.

## Don't
Don't commit a graph whose check still warns about a cited file you have not looked at: the citation may no longer say what the node claims. Don't refresh a pointer you did not write without reading it first (usage 220). Don't read the warnings as a failure: check still exits 0; `umx doctor` counts them in `doctor_graph_problems`.

## Example
```bash
umx check --graph launch
umx node evidence 01JC8XA4H7 refresh --at 1
umx resource refresh 01JD7K2M3N
umx check --graph launch
git add launch-graph && git commit -m "launch: May decisions" && git push
```

## Why
A graph in git is shared: teammates read its citations as true. Checking at the commit moment is the one time every changed source gets a person's look before it spreads.

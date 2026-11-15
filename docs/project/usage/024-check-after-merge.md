---
id: 024
title: After a merge, run umx check; repeated ids, dangling edges and unknown keys are kept and reported
route: doctor
status: current
source: req-67, req-76 (2026-11-10); architect rulings on the change
reqs: [req-67, req-76]
supersedes: 018
---
## Do
After a git merge (or whenever `umx graph load` warns), run `umx check --graph <alias>`. It reads the graph's files from disk and lists one row per finding: `duplicate_id` (the same node or edge id on several lines), `dangling_edge` (an edge whose `from` or `to` node is not in the graph) and `unknown_key` (a key under `ontology` that this umx does not know).

Know what each means while it is unfixed:
- Repeated id: the last line is the one in use. You can edit that node with the CLI as usual; every write keeps the other lines and repeats the `duplicate_id` warning.
- Dangling edge: it stays in edges.jsonl but is not an edge of the loaded graph (`edge list` and walks skip it). It joins the graph at the next load that finds its node. `kept_lines` in `umx graph list` counts these lines.
- Unknown key: kept as it is through every write.

To settle a repeated id or a dangling edge today (there is no delete verb yet): unload the graph, fix the file by hand (keep the one line you want; or restore the missing node's line, or remove the edge's line), then load it again and run `umx check` to confirm.

## Don't
Don't edit nodes.jsonl or edges.jsonl while the graph is loaded: the daemon's next write puts its own copy back. Don't treat a lower edge count after a load as lost data; read `kept_lines`. Don't delete keys you don't recognise.

## Example
```bash
umx graph load ~/work/launch-graph      # warns duplicate_id or dangling_edge
umx check --graph launch
umx graph unload launch
# edit the file: keep one line per id
umx graph load ~/work/launch-graph
umx check --graph launch                 # no rows
```

## Why
Nothing a merge produced is ever silently dropped or erased (req-67, req-76). A load warns at most 20 times and then counts the rest (`load_warnings_not_shown`); `umx check` always lists them all.

# Graphs in git

A graph folder is made to be committed. Saving an unchanged graph rewrites identical bytes, and a write changes
only the lines of the nodes and edges it changed, so `git diff` shows what changed: one JSON line per node or
edge, in id order. Commit `graph.json`, `nodes.jsonl`, `edges.jsonl`, `embeddings/` and `.gitignore`; the
`.umx-tmp-*` line keeps writes in progress out (`umx check` reports `gitignore_line_missing` if it goes).

## Unload before git changes the files

The daemon does not notice when git changes a loaded graph's files. After a checkout, pull, merge or rebase,
reads still show its old copy, and the next write puts that copy back over the new files, with no warning. So
unload first, and load again after:

    umx graph unload demo
    git pull
    umx graph load /path/to/my-graph
    umx check --graph demo

A graph you unloaded stays unloaded until you load it (`graph_not_loaded`); it does not reload on a restart.

## Merging two branches' edits

Ids are time-ordered, so nodes added on two branches all go at the end of `nodes.jsonl` and git reports a
conflict there. Keep both sides: delete only the `<<<<<<<`, `=======` and `>>>>>>>` lines, then commit. The same
goes for `edges.jsonl`. `graph.json` is one JSON document: keep both sides' entries and keep it valid JSON. Then
load the graph and check it:

    umx graph load /path/to/my-graph
    umx check --graph demo
    umx embeddings repair --graph demo    # when the graph has vectors

What the load and the check can report:

- `graph_invalid`: a conflict marker or a broken line is still in a file; the message names the file and line.
  Nothing loads until it is fixed.
- `duplicate_id`: one node or edge on two lines, because both branches edited it and both lines were kept. The
  last line is in use. Keep the line you want, delete the other, and load again.
- `dangling_edge`: an edge whose node was deleted on the other branch. It is kept in the file but is not in the
  graph. Delete the line, or bring the node back.
- `unsorted_file`: lines out of id order. It loads as usual, and the next write sorts the file, so that write's
  diff moves lines.

Unload the graph before editing a file by hand, and load it after. Which side of a real disagreement wins (two
titles, two statuses) is a person's call: ask. Every finding and how to settle it: `umx docs get graph`, Check.

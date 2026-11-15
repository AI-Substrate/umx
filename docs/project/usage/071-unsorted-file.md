---
id: 071
title: unsorted_file is a load warning; the next write of that file puts it in order
route: doctor
status: current
source: workshop 008 section 5.1 rule 2; workstream 003 follow-ups 2 (plan 036, 2026-11-10)
reqs: [req-02, req-67]
supersedes: none
---
## Do
When `umx graph load` warns `unsorted_file`, the named file (`nodes.jsonl` or `edges.jsonl`) has a line out of id
order or a blank line, first at the line it names. The graph loads and works as usual. Expect the next write that
touches that file to rewrite it in id order without blank lines, so its diff will move lines. If you want that
change on its own, make a small write now (for example `umx node note <id> --append "..."`) and commit the
reordered file by itself. `umx check --graph <alias>` lists it (row kind `unsorted_file`, `lines` = the first
line out of place) until a write fixes it.

## Don't
Don't sort the file by hand, and don't edit it while the graph is loaded. Don't treat it as damage: a git merge
that appends lines from both branches causes it, and nothing is lost.

## Example
```bash
umx graph load /Users/me/work/launch-graph
# warning unsorted_file: nodes.jsonl is out of id order or has a blank line, first at line 3. It loads as usual;
# the next write of it puts it in order.
umx check --graph launch     # row kind unsorted_file, file nodes.jsonl, lines [3]
```

## Why
umx writes files sorted by id so that diffs stay small and the same graph is always the same bytes (req-02). A
merge or a hand edit can leave lines out of order; umx accepts it rather than refusing the graph. It shares the
cap of 20 load warnings with `duplicate_id`, `dangling_edge` and `unknown_key` (`load_warnings_not_shown` counts
the rest).

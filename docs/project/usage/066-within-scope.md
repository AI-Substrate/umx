---
id: 066
title: Scope a search to a subtree with --within
route: explore
status: current
source: req-28; workshop 008 section 2.4; workstream 002 s11; plan 032
reqs: [req-28]
supersedes: none
---
## Do
When a question is about one part of a graph (a call, a workstream, a programme), add `--within <node>`: `umx node list`, `umx node find` and `umx walk` then return only nodes under that node by `part_of` (children point at their parent with `part_of`). For titles, `umx node find "<words>"` matches approximately, typos and word order allowed; to search notes and fields too, use `umx search "<words>" --within <node>` (usage 067).

## Don't
Don't expect `--within` to include the node itself, or nodes linked by other relations.

## Example
```bash
umx node find "approval delays" --within 01JC8X9Q2M
umx node list --within 01JC8X9Q2M --type problem
```

## Why
A subtree is how a concept graph groups work; scoping to it keeps answers about that part only.

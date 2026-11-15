---
id: 018
title: Repeated ids and dangling edges after a merge
route: doctor
status: superseded
source: req-67, req-76 (2026-11-10)
reqs: [req-67, req-76]
supersedes: none
superseded_by: 024
---
## Do
After a git merge, run `umx check`. A `duplicate_id` or `dangling_edge` is kept, not erased: fix it by editing through the CLI (keep one record, restore or delete the missing node, remove the edge).

## Don't
Don't hand-edit nodes.jsonl or edges.jsonl to fix it.

## Example
```bash
umx check
```

## Why
Nothing a merge produced is ever silently lost.

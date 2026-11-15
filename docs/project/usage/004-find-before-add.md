---
id: 004
title: Look before you add a node
route: build
status: current
source: user interview 2026-11-09; req-57
reqs: [req-57]
supersedes: none
---
## Do
Run `umx node find "<words>"` before `umx node add`. If `node add` returns `near_duplicate`, check the candidate with the `next` command and reuse it unless it is genuinely different.

## Don't
Don't add a node per mention. Don't ignore near_duplicate hints.

## Example
```bash
umx node find "approval delay"
umx node add --type problem --title "Approval delays"
```

## Why
Duplicates were the main friction in live graphs; most later passes should change nodes, not add them.

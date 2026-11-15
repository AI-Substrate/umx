---
id: 016
title: Addresses, copies and the repo hint
route: multi-graph
status: planned
source: D25 (2026-11-10); workshop 002 s7; req-14, req-69
reqs: [req-14, req-15, req-69]
supersedes: none
---
## Do
Paste the copy-free `address` (umx:<alias>/<id>) into documents. Use `umx:<alias>@<copy>/<id>` only as a working note for a worktree copy. Add `?repo=<git url>` when the reader may not have the graph; it is a fetch hint only.

Until req-69 is built, the `@<copy>` form does not resolve: load each copy under its own alias
(`umx graph load <path> --as acme_feature`) and address it as `umx:acme_feature/<id>`. `umx resolve` refuses
`umx:<alias>@<copy>/<id>` with usage_invalid. `?repo=` works now: see usage 105.

## Don't
Don't paste @copy addresses into lasting documents (they stop resolving when the worktree goes).

## Example
```bash
umx:acme/01JCB8ZK...?repo=git@github.com:org/acme-graph.git
```

## Why
Ids are identity; alias, copy and repo are routing hints.

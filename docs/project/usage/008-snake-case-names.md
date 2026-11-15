---
id: 008
title: Names are snake_case
route: build
status: current
source: verb grammar 007 rule 2; Cypher POC 006
reqs: [req-27]
supersedes: none
---
## Do
Types, statuses, relations and fields are `[a-z][a-z0-9_]*`: `depends_on`, `conflicts_with`. Cypher keywords (match, group, ...) are refused with a suggested `<name>_item`.

## Don't
Don't use hyphens or capitals in names.

## Example
```bash
umx edge add <a> depends_on <b>
```

## Why
One spelling everywhere: CLI, JSON, files and Cypher labels.

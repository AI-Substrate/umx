---
id: 027
title: evidence_kinds is computed from a node's evidence
route: query
status: current
source: blueprint 001 E-6 (ap-14); workshop 007 rule 4
reqs: [req-18]
supersedes: none
---
## Do
Ask for `evidence_kinds` as a column to see where a node's evidence came from (the distinct `room`, `chat`, `screen`, `document`, `analyst` kinds in its evidence, sorted). Add evidence with `umx node note ... --evidence <pointer> --evidence-kind <kind>`.

## Don't
Don't try to set `evidence_kinds` yourself; it is a reserved, read-only field.

## Example
```bash
umx node list --type problem --columns id,title,evidence_kinds
```

## Why
One place computes it, so every reader (CLI, a later UI, queries) gets the same answer.

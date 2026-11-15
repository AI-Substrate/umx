# Usage notes: how agents should use umx

The source the `/umx` skill and `umx docs` are built from. Every ruling, design choice or PR that changes
HOW an agent should use umx adds a note here, so the detail is never reverse engineered later.

## Rules
- One note per file: `NNN-<slug>.md`. Take the number from the project's number-claim tool (numbers 001 to 050
  were reserved before claims were used, so claims start at 051). Never pick a number by looking at the folder. Notes are never deleted; a changed rule gets a new note that says
  which note it supersedes, and the old note gets a `superseded_by` line.
- Who writes: whoever makes the ruling or ships the behaviour, in the same PR (the project lead for project decisions,
  experts for their designs, coders for behaviour they ship). Reviewers check a PR that changes usage has one.
- Plain language, no em dashes. Show real commands. Say when to do it AND when not to.

## Note format
```
---
id: NNN
title: <one line>
route: start | build | ingest | explore | query | similarity | resources | multi-graph | doctor | output
status: current | planned (behaviour not built yet) | superseded
source: <ruling id and date, workshop section, PR>
reqs: [req-NN, ...]
supersedes: <id or none>
---
## Do
## Don't
## Example
## Why
```

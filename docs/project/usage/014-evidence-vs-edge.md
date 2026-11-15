---
id: 014
title: Evidence or a selector edge?
route: resources
status: planned
source: D35 (2026-11-10)
reqs: [req-18, req-78]
supersedes: none
---
## Do
Use evidence for where a node's content came from ("this claim was said at 3206-3240"). Use a selector edge for a typed relationship to a passage ("this answer was given in these lines", "this line cites that document").

## Don't
Don't use both for the same fact unless the relationship itself matters.

## Example
```bash
evidence: --evidence T.ndjson#seq=3206-3240
edge:     umx edge add <answer> answered_in <T>#seq=3206-3240
```

## Why
Provenance and relationships answer different questions.

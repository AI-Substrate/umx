---
id: 007
title: The vocabulary is soft: extend it, don't fight it
route: build
status: current
source: D7, D11 (2026-11-09); req-10
reqs: [req-08, req-10]
supersedes: none
---
## Do
When a write warns `off_vocabulary_type` (or status, relation, field), decide: if the new name is right for this graph, run the `umx ontology add ...` command in the warning's `next`. Shape the vocabulary right after `umx graph new` (it prints how).

## Don't
Don't contort content to fit the starter vocabulary. Don't leave many off-vocabulary warnings unresolved.

## Example
```bash
umx ontology add type constraint
umx ontology add relation defined_in
```

## Why
The tool informs, never polices (D11, "we are not the police").

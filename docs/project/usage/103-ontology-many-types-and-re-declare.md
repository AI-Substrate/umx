---
id: 103
title: Add one field to many types in one call; re-running an identical ontology add is ok
route: build
status: current
source: F71 (field report, 130 calls for one field), project manager additions from two testers 2026-11-11, architect ruling on the wire shape; plan 073
reqs: [req-08]
supersedes: none
---
## Do
- To put one field on several types, name them all in one call: `umx ontology add field risk,issue,task score:int`
  (a comma list with no spaces, or several types before the field). To put it on every type the graph declares,
  use `umx ontology add field --all-types reference:string`. It is one write, all or nothing.
- If it is refused, read every error: each culprit has its own. `type_unknown` names a type the graph does not
  declare (declare it with its next, or drop it from the list). `field_conflict` names a type that already has the
  field with another kind. Nothing was written; fix the list and run it again.
- Read `data.types` (the types that got the field) and `data.unchanged` (types that already had it with the same
  kind and were left alone).
- Re-run setup scripts freely. An `ontology add` that matches what is already declared answers ok with
  `"unchanged": true` and writes nothing:
  - a field with the same kind;
  - a status;
  - a relation with the same `--symmetric` flag (so `umx ontology add relation conflicts_with --symmetric` is
    unchanged);
  - a type, when it already has every `--field` you give with the same kind (fields you do not give are fine).

## Don't
- Don't loop `umx ontology add field` over types; one call does it.
- Don't expect the list or `--all-types` form to declare new types. Only the one-type form
  (`umx ontology add field brand_new size:int`) declares a type that is not there yet.
- Don't mix a type list and `--all-types` (`usage_invalid`).
- Don't treat a differing re-declare as a no-op. These are refused and change nothing:
  - a field with another kind: `field_conflict`;
  - `umx ontology add type risk --field score:int` when risk is declared without `score`: `field_missing`. Its
    next is the `umx ontology add field risk score:int` that adds it. There is no partial add;
  - a plain `umx ontology add relation conflicts_with`: it is symmetric (built in), so it is `already_declared`.

## Example
```bash
umx ontology add field problem,decision,option evidence_text:string
umx ontology add field --all-types owner:string
umx ontology add field --all-types owner:string   # again: ok, "types": [], every type under "unchanged"
umx ontology add status parked                    # again: ok, "unchanged": true
```

## Why
A tester made 130 calls to add one field everywhere, and its setup scripts failed on re-run with
`already_declared` for declarations that were already exactly right. One call and one write keep a field change
atomic, and a no-op re-declare makes setup idempotent while a real difference is still refused.

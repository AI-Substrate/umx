---
id: 083
title: Symmetric relations are one edge both ways; add once, follow from either end
route: build
status: current
source: decision D49 (2026-11-10), F48; PR for plan 048
reqs: [req-27, req-28]
supersedes: none
---
## Do
- Add a `conflicts_with`, `alternative_to` or `possibly_same_as` edge once, in either direction. umx stores it with the lower id as `from`, and adding it the other way returns the same edge with `edge_exists`.
- Mark your own symmetric relation with `umx ontology add relation <r> --symmetric`. `umx ontology show` lists every symmetric relation under `symmetric`.
- Expect `umx neighbours`, `umx walk` and `umx path` to follow a symmetric edge from either end whatever `--direction` says; `way` shows how it is stored.
- After a merge, a `symmetric_duplicate` warning (or `umx check` row) means both directions are in edges.jsonl. Settle it as for `duplicate_id`: unload, delete the named line, load, check.

## Don't
- Don't add the reverse of a symmetric edge to "make it two-way": it already is.
- Don't read `umx edge list --from/--to` as symmetric: it lists edges as stored.
- Don't mark a directed relation (`blocks`, `depends_on`) symmetric: its direction is its meaning. `part_of` and `similar_to` are refused (`symmetric_refused`).
- Don't be surprised when marking a relation warns `symmetric_duplicate`: edges you already had both ways are now one edge, and the other line stays in edges.jsonl until you settle it.

## Example
```bash
umx edge add 01JC8XB2KQ conflicts_with 01JC8XA4H7   # stored 01JC8XA4H7 conflicts_with 01JC8XB2KQ
umx ontology add relation rivals --symmetric
umx neighbours 01JC8XB2KQ --direction out           # includes 01JC8XA4H7
```

## Why
One stored edge per symmetric fact keeps counts right (ws005's Cypher counts depend on it) and makes "who conflicts with X" one question, not two.

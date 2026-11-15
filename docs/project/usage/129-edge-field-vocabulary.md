---
id: 129
title: Declare edge fields on their relation; an undeclared edge field warns like a node field
route: build
status: current
source: F92 (field report 2026-11-11, via the project lead), the graph-addressing expert's storage note (relation_fields, reserved edge keys), the project manager on the wire shape; plan 100
reqs: [req-08, req-13]
supersedes: none
---
## Do
- Put a relationship's attributes on the edge with `--set`: `umx edge add <a> supports <b> --set confidence=0.7`.
- When the edge add warns `off_vocabulary_edge_field`, and the field is right for this graph, run its `next`:
  `umx ontology add field --relation supports confidence:float`. That declares the field on the relation's edges
  (and the relation too, if it is new). Later values are typed by that kind.
- Read the declared edge fields with `umx ontology show` (`relation_fields`, by relation).
- Re-run setup scripts freely: declaring the same relation field with the same kind again is ok with
  `"unchanged": true` and writes nothing.
- A batch `edge add` line gets the same checks and warnings, each at its line.
- `umx edge edit <edge> --relation <r>` moves the edge's custom fields to the new relation: each one it does not
  declare warns `off_vocabulary_edge_field` (the same in a batch `edge edit` line). Declare them on the new
  relation with the warning's next.

## Don't
- Don't declare an edge field with `umx ontology add field <type> ...`: that declares it on a node type. Use
  `--relation`.
- Don't use an edge's own keys as field names (`id`, `type`, `from`, `to`, `from_graph`, `to_graph`,
  `from_selector`, `to_selector`, `evidence`, `evidence_kinds`, `evidence_removed`, `std`, `created`,
  `created_by`, `updated`, `updated_by`, provenance keys):
  `reserved_name`.
- Don't change a declared field's kind by declaring it again: another kind is refused with
  `relation_field_conflict`, and nothing changes.
- Don't declare fields on a derived relation (`similar_to`, `authored_by`, `edited_by`: `name_invalid`); umx
  makes or derives those edges itself.

## Example
```
umx edge add 01JC8XA4H7 supports 01JC8XB2KQ --set confidence=0.7
# warning off_vocabulary_edge_field: Field confidence is not declared on relation supports.
umx ontology add field --relation supports confidence:float
# ok: { "added": "relation field", "relation": "supports", "name": "confidence", "unchanged": false }
umx edge add 01JC8XB2KQ supports 01JC8XC9MN --set confidence=0.9   # ok, no warning
umx ontology show                                                  # "relation_fields": { "supports": [...] }
```

## Why
Edges are first class (req-13) and every graph has its own vocabulary (req-08). Before F92, edge fields had no
vocabulary at all. A tester graph put confidence and tenure on edges with no warning, and no way to declare
their kinds. Declared relation fields are kept in graph.json only when there are some, so older files are
unchanged and older umx builds keep the key as they found it. The std vocabulary (ws008) declares its
relation fields the same way (for example starts_on and ends_on on works_for).

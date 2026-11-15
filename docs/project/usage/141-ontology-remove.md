---
id: 141
title: Remove an ontology entry only once nothing uses it; built-ins never go
route: build
status: current
source: F76 (split from F71, field reports from two testers 2026-11-11), architect ruling 2026-11-12 (one Op per form, built-ins, one census pass); plan 111
reqs: [req-08]
supersedes: none
---
## Do
- Take a type, status, relation or field out of a graph's vocabulary with `umx ontology remove`:
  `umx ontology remove type risk`, `... status parked`, `... relation mitigates`, `... field risk impact`, and for
  a field declared on a relation's edges `... field --relation works_for starts_on`.
- If it refuses with `ontology_in_use`, run its next to see the nodes or edges that still use the entry (a type's
  nodes, a status's nodes, a relation's edges, the nodes of a type that hold the field, or the relation's edges
  that hold it). A field the type's default columns name refuses too; its next resets them
  (`umx ontology set type <t> --reset`). Change or delete them,
  then remove again.
- Re-run setup scripts freely: removing what is not declared is ok with `"unchanged": true` and writes nothing.

## Don't
- Don't expect a removal to change any node or edge: it only changes the vocabulary, and it refuses while records
  use the entry.
- Don't count on kept lines to block a removal: only records in the graph as loaded count. A duplicate id's other
  lines and dangling edges stay in the files, and after the removal they warn off-vocabulary when the graph loads
  (`umx check` lists them).
- Don't try to remove what umx relies on (`ontology_builtin`): `part_of`, the default status `open`, the derived
  relations (`similar_to`, `authored_by`, `edited_by`), the provenance types and relations, `same_as`, the
  standard node fields, or any `std:` entry.
- Don't remove a type to rename it: there is no rename. Add the new type, move the nodes with
  `umx node edit <node> --type <new>`, then remove the old one.

## Example
```
umx ontology remove type problem
# error ontology_in_use: type problem is used by 12 nodes in launch; nothing changed.
# next: umx node list --type problem --graph launch
umx ontology remove type capability
# ok: { "removed": "type", "name": "capability", "unchanged": false }
```

## Why
Each graph owns its vocabulary (req-08), and agents asked to tidy entries a setup script declared but the graph
never used. Refusing while records use an entry keeps every node and edge on a declared name it already had, so a
removal never turns existing data off-vocabulary. umx's own entries stay because umx writes or derives them.

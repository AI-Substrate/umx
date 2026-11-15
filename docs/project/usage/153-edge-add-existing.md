---
id: 153
title: An existing edge add never drops new field values
route: build
status: current
source: plan 125; graph-addressing edge-add-existing ruling, 2026-11-12
reqs: [req-13, req-88]
supersedes: none
---
## Do

Re-run an edge add when it supplies no fields or every supplied field already matches its stored typed value.
It returns the existing edge with `edge_exists`, without writes or stamps. Identity uses the relation and
graph-id/node-id endpoints in the storing graph; aliases and named copies resolve to the same identities.
Symmetric ends may be swapped. Stored passage selectors distinguish passage edges from whole-node edges.

On `edge_exists_differs`, the error gives the existing edge id and missing or different field names, never
values. To change those fields, edit the edge in place, keeping its id: `umx edge edit <edge> --set
<field>=<value>` (`--set-json` or `--set-str` to keep a value's exact type) and `--unset <field>` (usage 189).

## Don't

Do not use add to edit or merge fields. There is no `--merge`. Change an existing edge's fields with
`umx edge edit <edge> --set <field>=<value>` and `--unset <field>` (usage 189); the edge keeps its id.
Do not write selector fragments or reserved `from_selector` / `to_selector` fields.
Do not retry a refused batch as single writes: any failed line leaves the whole batch unchanged.

## Example

    umx edge add 01JC8XA4H7 supports 01JC8XB2KQ --set confidence=0.7
    umx edge add 01JC8XA4H7 supports 01JC8XB2KQ --set confidence=0.7
    # edge_exists, unchanged
    umx edge add 01JC8XA4H7 supports 01JC8XB2KQ --set confidence=0.8
    # edge_exists_differs, naming the edge; no values changed
    umx edge edit <that edge> --set confidence=0.8
    # changed: ["confidence"], same id

In a batch, the same refusal names its line in dry and real runs and rolls back every earlier line; use an
`edge edit` line with `sets` there instead.

## Why

An idempotent retry must not silently discard another reader's explanation or other new fields.
Add either preserves the existing edge exactly or refuses; it never chooses how to combine field values.

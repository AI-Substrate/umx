---
id: 152
title: Distinguish unused query vocabulary and use type only on relationships
route: query
status: current
source: cypher-expert query-declared-unused and cypher-type-alias notes, plan 124
reqs: [req-27]
supersedes: none
---
## Do

Read `query_label_unused`, `query_relation_unused` and `query_property_unused` as soft warnings: an ontology
in the selected scope declares the name, but no node or edge uses it yet. The query still returns its normal
empty rows or null fields. Run `umx ontology show` to inspect declarations. A property is declared only when
a node type or relation declares that field. Declarations form a union across the graphs selected for this call,
not graphs outside the scope.

Use `type(r)` or `label(r)` for a single-hop relationship variable, including `similar_to`. Both return the
relation name, or null for an unmatched OPTIONAL MATCH. This is a documented umx departure: LadybugDB 0.22.3
rejects `type(r)`, so its conformance twin uses `label(r)`.

## Don't

Do not treat an unused warning as a typo or failed query. `query_unknown_label`, `query_unknown_relation` and
`query_unknown_property` still identify names not declared or used in the scope. Do not widen scope to named or
secured graphs just to silence a warning.

Do not use `type(n)` for a node, `type(r)` on a variable-length relationship list, `labels(n)` or `id(n)`.
Use `label(n)` and `n.id`. To read each relationship's name, query a single-hop binding with `label(r)` or
`type(r)`; relationship-list UNWIND is not supported.

## Example

    umx query 'MATCH (q)<-[:answers]-() RETURN q.id AS id' --graph notes
    umx ontology show --graph notes
    umx query 'MATCH ()-[r:answers]->() RETURN type(r) AS relation' --graph notes
    umx query 'MATCH (n) RETURN label(n) AS kind, n.id AS id' --graph notes

If `answers` is declared but has no edges, the first query returns empty rows with `query_relation_unused`,
not `query_unknown_relation`. Both warnings point to `umx ontology show`.

## Why

An empty graph is not an unknown vocabulary. Keeping the warning soft preserves query results while making
the next action accurate. The relationship-only alias supports familiar Cypher spelling without pretending
that node types, relationship lists or LadybugDB have the same function.

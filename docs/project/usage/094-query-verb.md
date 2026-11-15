---
id: 094
title: Ask the graph with umx query, a read-only Cypher subset
route: query
status: superseded
superseded_by: 095, 096
source: workstream 005 s04 (plan 061, req-27); cypher-expert design input section 8; D49
reqs: [req-27]
supersedes: none
---
## Do
Use umx query for questions that need joins, counts or conditions across edges:

    umx query 'MATCH (q:question) WHERE NOT EXISTS { MATCH (q)<-[:answers]-() } RETURN q.id AS id, q.title AS title ORDER BY id'

Put values in parameters, and quote the query with single quotes so the shell leaves `$id` alone:

    umx query 'MATCH (n {id: $id})<-[:blocks*1..3]-(b) RETURN DISTINCT b.id AS id' --param id=<id>

Return fields (n.id, n.title), give every column an alias that is not a variable name, and bound every
variable-length step (*1..10 at most). Use label(r) for an edge's relation, <> for not-equal.
Symmetric relations have no direction: match them with -[:r]-, and add WHERE a.id < b.id to list each pair once.
Name graphs with --graph a,b to include secured ones; with none named, only open graphs are read.
Read `total` and `truncated`: at most 1,000 rows come back unless you pass --limit (up to 100000).

## Don't
Don't write: the subset is read-only. Use umx node add, umx node edit and umx edge add to change the graph.
Don't use Cypher for shortest paths or one node's neighbourhood: umx path, umx neighbours and umx walk do it directly.
Don't paste titles or other graph text into the query; use --param.
Don't name similar_to in a query yet: it is refused until it is built. Use umx neighbours <node> --relation similar_to.
Don't retry a query_too_large or query_timed_out unchanged: bind the start node by id, lower the bound, or add LIMIT.

## Example
    umx query 'MATCH (d:decision)<-[:blocks]-(b) RETURN d.id AS id, count(*) AS blockers ORDER BY blockers DESC LIMIT 10'

Each refusal has a stable code and says what to write instead; `umx docs get query` lists them.

## Why
The subset is exactly what umx and LadybugDB answer the same way, so a query that works today keeps working if
the graph later moves to a database. Parameters keep graph text out of the query, so a hostile title can never
change what a query does. Every refusal names a rewrite.

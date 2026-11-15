---
id: 095
title: umx query reads each graph once, and parameters can be strings or lists
route: query
status: superseded
superseded_by: 096
source: ws005 query follow-ups (plan 063); joint ruling d-21 (graph-addressing-expert and architect, 2026-11-11); cypher-expert review F1
reqs: [req-27, req-69]
supersedes: 094
---
## Do
Use umx query for questions that need joins, counts or conditions across edges:

    umx query 'MATCH (q:question) WHERE NOT EXISTS { MATCH (q)<-[:answers]-() } RETURN q.id AS id, q.title AS title ORDER BY id'

Put values in parameters, quoting the query with single quotes so the shell leaves `$id` alone:

    umx query 'MATCH (n {id: $id})<-[:blocks*1..3]-(b) RETURN DISTINCT b.id AS id' --param id=<id>

Use --param-str when graph text must stay text (a title "2026" or "true"); --param types it like --set:

    umx query 'MATCH (n) WHERE n.title = $t RETURN n.id AS id' --param-str t=2026

Use --param-json for a list, for `IN $ids` (a flat list of values only):

    umx query 'MATCH (n) WHERE n.id IN $ids RETURN n.title AS title' --param-json 'ids=["<id>","<id>"]'

Return fields (n.id, n.title), give every column an alias that is not a variable name, and bound every
variable-length step (*1..10 at most). Use label(r) for an edge's relation, <> for not-equal.
Symmetric relations have no direction: match them with -[:r]-, and add WHERE a.id < b.id to list each pair once.
Read `total` and `truncated`: at most 1,000 rows come back unless you pass --limit (up to 100000).

When the same graph is loaded twice (two worktrees, two aliases), a query that names no graph reads it once, from
the copy loaded first, and says `copies_skipped` with the others. Name copies with --graph a,b only when you want
both; then counts include each copy (`query_graph_twice`) and whole nodes say their `copy`. If you see
`copy_ambiguous`, name the copy you mean with --graph. umx search reads copies the same way. Walk, path, neighbours
and node list read one graph: with no --graph they read the primary copy (with copies_skipped); name one copy with
--graph, never a comma list.
A variable must be bound by a MATCH pattern, UNWIND or WITH before it is read; `RETURN nope.id` is refused
(query_unknown_variable), never answered with nulls.

## Don't
Don't write: the subset is read-only. Use umx node add, umx node edit and umx edge add to change the graph.
Don't use Cypher for shortest paths or one node's neighbourhood: umx path, umx neighbours and umx walk do it directly.
Don't paste titles or other graph text into the query; use --param-str.
Don't name similar_to in a query yet: it is refused until it is built. Use umx neighbours <node> --relation similar_to.
Don't retry a query_too_large or query_timed_out unchanged: bind the start node by id, lower the bound, or add LIMIT.
Don't add counts from two copies by hand: name one copy, or read them once with no --graph.

## Example
    umx query 'MATCH (d:decision)<-[:blocks]-(b) RETURN d.id AS id, count(*) AS blockers ORDER BY blockers DESC LIMIT 10'

Each refusal has a stable code and says what to write instead; `umx docs get query` lists them.

## Why
The subset is exactly what umx and LadybugDB answer the same way, so a query that works today keeps working if
the graph later moves to a database. Parameters keep graph text out of the query, so a hostile title can never
change what a query does; --param-str keeps text that looks like a number or a boolean text. Reading one copy per
graph keeps counts the same whether a graph is loaded once or from several worktrees, and agrees with how a bare id
resolves with no graph named; an address that names an alias reads that copy (d-21).

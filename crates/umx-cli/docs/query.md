# Query

`umx query` asks the graph a question that needs joins, counts or conditions across edges, in a read-only subset
of Cypher. Give the query as one quoted argument; put any value from the graph or the user in a parameter.

    umx query 'MATCH (q:question) WHERE NOT EXISTS { MATCH (q)<-[:answers]-() } RETURN q.id AS id, q.title AS title ORDER BY id'
    umx query 'MATCH (n {id: $id})<-[:blocks*1..3]-(b) RETURN DISTINCT b.id AS id' --param id=<id>
    umx query 'MATCH (d:decision)<-[:blocks]-(b) RETURN d.id AS id, count(*) AS blockers ORDER BY blockers DESC' --graph demo,ops --limit 20

The answer is a list: `columns` in RETURN order, `rows` (one object per row, keyed by column), `total` (rows the
query made) and `truncated`. `work` says how much of the work budget the query used; the same query on the same
graphs always uses the same amount.

## Provenance

Provenance nodes and incident edges are hidden by default, including from counts and intermediate
steps. `--provenance` includes them inside the graph scope already selected, never another graph: provenance
shows only in the graphs the read names (every graph read when it names none), so name a far graph to see its
provenance.
Provenance is a record of what the writer claimed, never proof or permission.

    umx query 'MATCH (n)-[r:authored_by]->(s:session) RETURN n.id AS node_id, s.id AS session_id, r' --provenance

`authored_by` and `edited_by` are derived from a node's session fields, not stored edges. They point
from node to session; `session_of` leads from session to agent. They are never symmetric, stay inside
their node's graph, and work in either direction. A missing session gives no row.
Like `similar_to`, they must be named in the pattern: untyped steps never follow them, even with
the flag. Unlike `similar_to`, variable-length steps on them are refused with
`query_feature_not_supported`; write a single hop.

`RETURN r` gives `{id: null, type, from, to, graph, derived: true}`, with `copy` when named copies
are read. There is no address; `r.id` and `r.address` are null. `r.graph` names the node's graph.
An edge's attribution is in `r.authored_by_session` and `r.edited_by_session`, never a derived step.

Without the flag, a query naming a reserved provenance label (`session`, `agent`, `model`, `harness`
or `platform`), `authored_by`/`edited_by`, or a stored provenance relation (`session_of`, `runs_on`,
`uses_model`, `registered_in`) warns `provenance_hidden`, not `query_unknown_*` or
`query_*_unused` for that name. This warning depends only on the name as a label or a step's type in the
query, even when no provenance exists; the same name as a variable or a map key warns nothing. An ordinary user
node of one of those types stays visible.

## std names

std names are plain in Cypher: write `(p:person)` and `-[:member_of]->`, never `std:`. A label or relationship
type spelled `std` or `std:<name>` (`(n:std:person)`, ``(n:`std:person`)``, `-[:std:member_of]-`) is refused with
`query_std_qualifier` ("std names are plain in Cypher: write :person"), because `(n:std:person)` is a label union
that would otherwise widen to every node. `std` as a variable or a property is a name like any other.

## The subset

- Clauses: `MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `UNWIND`, `RETURN`, with `DISTINCT`, `ORDER BY`, `SKIP`
  and `LIMIT`. A query starts with `MATCH`, `OPTIONAL MATCH`, `UNWIND` or `WITH`.
- Patterns: labels are node types (`(n:question)`, `(n:question|criterion)` for either), relations are edge
  types (`-[:blocks]->`, `-[:supports|undercuts]->`), `<-` and `-` for incoming and either way, a property map
  (`{id: $id}`), and a variable-length step with an upper bound of at most 10 (`-[:depends_on*1..4]->`).
- Conditions: `=`, `<>`, `<`, `<=`, `>`, `>=`, `AND`, `OR`, `NOT`, `IN`, `IS NULL`, `IS NOT NULL`,
  `STARTS WITH`, `ENDS WITH`, `CONTAINS`, `EXISTS { MATCH ... WHERE ... }`, and `CASE WHEN ... THEN ... ELSE ... END`.
- Functions: `toLower`, `toUpper`, `size`, `coalesce`, and `label(x)` for a node's type or an edge's relation.
  `type(r)` is an alias of `label(r)` only for a single-hop relationship variable, including similar_to.
  Both return null for an unmatched OPTIONAL MATCH. `type(n)` and a variable-length relationship list are
  refused; use `label(n)` for nodes and a single-hop binding to read each edge's relation.
  Aggregates: `count`, `count(DISTINCT ...)`, `sum`, `avg`, `min`, `max`, `collect`.
- Date functions (`umx docs get time` has the rules): `date(x)`, `timestamp(x)`, `current_date()`,
  `current_timestamp()`, `date_part(unit, x)`, `date_trunc(unit, x)` and `date_diff(unit, a, b)`, in universal
  time. Null or text that is not a date gives null, and so does a part or cut finer than the value
  (`date_part("day", "2025-01")`). `date_diff` counts boundaries, except week: whole 7-day spans, as DuckDB's
  `date_diff` does (`date_diff("month", "2025-01-31", "2025-02-01")` is 1, `date_diff("week", <Sunday>, <Monday>)`
  is 0).
  Departures from LadybugDB: `date` also takes a month, a year or a date-time where LadybugDB
  wants `YYYY-MM-DD`, `timestamp` converts an offset where LadybugDB drops it, `date_part("hour", <a day>)` is null
  where LadybugDB says 0, and `date_diff` is umx's own (LadybugDB writes the day gap `date(b) - date(a)`, which umx
  does not have). `now()` and `datetime()` are not functions here: use `current_timestamp()` and `timestamp(x)`.
- Fields: `n.id`, `n.type`, `n.title`, `n.status`, `n.note`, `n.created`, `n.updated`, `n.created_by`,
  `n.evidence_kinds`, `n.address`, `n.graph`, `n.tags` (in a std graph its tags in stored form, so
  `'needs_review' IN n.tags` matches as `--tag` does; a node with no tags reads null) and any custom field by name; on an edge, `r.id`, `r.from`, `r.to`,
  `r.address`, `r.graph` and its fields; on a similar_to edge, `s.score`.
- `NOT` is two-valued here: `NOT 'x' IN n.tags` keeps nodes whose `n.tags` is null (no tags) as well as nodes
  whose tags leave `x` out. For "nodes without `x`" write `n.tags IS NULL OR NOT 'x' IN n.tags`, which says so
  and keeps the same rows if `NOT` ever follows openCypher's three-valued rule (where null stays null and the row
  is dropped).
- Resource nodes (`umx docs get resource`) answer `n.locator_kind`, `n.locator_source`, `n.locator_format`,
  `n.locator_hash`, `n.locator_length` and `n.locator_seq_max` (null on a plain node). `n.locator` is not a
  property: it reads null with `query_unknown_property`.
- Walks, not trails: a variable-length step may pass an edge twice, and parallel edges are separate rows, as in
  LadybugDB. Rows with no ORDER BY come in graph order, then id order, the same on every run.

`type(r)` is a documented umx departure: LadybugDB 0.22.3 rejects the function. Its conformance twin uses
`label(r)`, and a checked probe detects if LadybugDB starts accepting `type(r)`. Variables and aliases named
`type` remain legal.

Return fields (`n.id`, `n.title`), and give every column an alias that is not a variable name: in
`MATCH (a)-[:blocks]->(b) RETURN b.id AS a`, `a` is already a node, so it is refused with
`query_alias_shadows_variable`. Name columns for what they hold: `AS blocker_id`, `AS node_id`, `AS title`. A whole node in
RETURN comes back as `{id, address, graph, type, title, status}`; a whole edge as
`{id, address, graph, type, from, to}` plus `from_selector` and `to_selector` when set. Name a field to get more.

### Comparing dates

Two strings that both read as dates compare as dates, not as text: a umx departure from openCypher, which compares
strings as text. The rules are `--where`'s, from one function: `<` wholly before, `>` wholly after, `=` overlapping,
`<=` and `>=` either (table in `umx docs get time`). They hold for `WHERE`, `<>`, property maps, `IN`
and the simple `CASE`. `ORDER BY`, `min` and `max` sort as before (stored dates sort by start, the longer span
first, as their text does), except that a date written another way sorts at its UTC form.

    umx query 'MATCH (m:meeting) WHERE m.starts_on = "2026-10-29" RETURN m.title AS title'   # any time that UTC day
    umx query 'MATCH (a:action) WHERE a.due_on < "2025-02" RETURN a.title AS title'          # wholly before February

A literal with an offset compares as its instant (`"2026-10-30T09:40:00+11:00"`). Write a year as text (`"2025"`,
`--param-str y=2025`): the number `2025` is never a date. `DISTINCT`, grouping and lists compare dates as text, so
`2025` and `2025-01` stay two values, and `["2025"] = ["2025-01"]` is false.

## Parameters

`--param name=value` gives `$name` in the query. Each value is typed as `--set name=value` types it: a whole
number, a decimal number, `true` or `false`, else a string. `--param-str name=value` is always a string, so a title
such as `2026` or `true` matches as text; `--param-json name=<json>` takes a value or a flat list, for `IN $ids`.
Names are snake_case and given once. A parameter is the safe way to put a title or any other graph or user text
into a query; never paste it into the query text. A column that returns a parameter unaliased is named `$name`.

    umx query 'MATCH (n) WHERE n.title = $t RETURN n.id AS id' --param-str t=2026
    umx query 'MATCH (n) WHERE n.id IN $ids RETURN n.title AS title' --param-json 'ids=["<id>","<id>"]'

## Symmetric relations and similar_to

Symmetric relations (`conflicts_with`, `alternative_to`, `possibly_same_as`, `same_as`, `not_same_as` and those marked `--symmetric`) have no
direction: match them with `-[:conflicts_with]-`, and add `WHERE a.id < b.id` to list each pair once. A directed
step on one is refused (`query_symmetric_direction`). An untyped directed step that crosses one runs and warns
(`query_symmetric_in_directed_step`).

similar_to is followed only when a step names it: an untyped step (`-[]-`), `count(r)` over `-[r]-` and
`(n)--()` never include it. `(a)-[s:similar_to]->(b)` means b is in a's top k (`umx neighbours a --relation
similar_to`, out); `<-[:similar_to]-` lists the nodes whose top k holds a. Treat it as a ranking, not a fact.

    umx query 'MATCH (a:question)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN a.id AS a_id, b.id AS b_id, s.score AS score'
    umx query 'MATCH (a {id: $id})-[s:similar_to]->(b) RETURN b.title AS title, s.score AS score' --param id=<id>

- `s.score` is a number from -1 to 1, rounded to four decimals, and never below the graph's floor; `label(s)` is `similar_to`; `s.id` and `s.address` are
  null, because the edge is derived from the lists, not a record. `RETURN s` gives
  `{id: null, address: null, graph, type: "similar_to", from, to, score, derived: true}`, plus `copy` when copies of
  one graph are named. `derived: true` means
  it is not a stored edge, so `umx edge list` never shows it and no edge verb can name it; name it by its
  two ends. Human output shows it as
  `<from> -[similar_to <score>]-> <to>`.
- A variable-length step that names similar_to, alone or in a list such as `[:similar_to|blocks*1..2]`, goes at
  most 2 steps (`query_bound_too_large`).
- An unbound start (`MATCH (a)-[s:similar_to]->(b)`) reads every node's list, about k entries per node.
- The answer then carries `similarity`: one entry per graph queried that holds lists, `{graph, model, k, floor,
  floor_calibrated, floor_source, floor_run}`. A calibrated similar_to floor is the score only 1 in 100 unrelated document pairs reach;
  while `floor_calibrated` is false the floor only bounds what the lists keep, it is not a relevance cut. `similar_to_incomplete` warns when the lists are being rebuilt, or a similar_to step read a
  node that has no vector yet (its list is empty); run `umx similarity rebuild --graph <alias>` if it stays.
- Steps stay inside each graph: similar_to never links two graphs.

## Which graphs

- A plain query already reads every loaded graph with open visibility when no graph is named.
  `--all` makes that choice explicit and overrides `UMX_GRAPH`; it uses one primary copy per graph id.
- `--all --include-named` also reads named graphs for this call only. `--include-named` requires
  `--all`, and `--all` cannot be combined with `--graph` or `--scope`. No environment variable,
  config or saved scope enables the opt-in.
- Use `--include-named` only when the user asked for named or secured graphs in this task; never as
  a default, and never as a retry for empty results. `secured_graphs_read` names the graphs read:
  named text is now in this output and stays on this machine only if the caller keeps it there.
- `--scope both` queries a saved scope instead of `--graph` (`umx docs get scope`).
- `--graph demo` (or `UMX_GRAPH`) queries one graph; `--graph demo,ops` queries those. A graph you name is
  queried whatever its policy. A list explicitly names every graph it contains, including named/secured graphs.
- With no graph named, every loaded graph whose policy is open. Secured graphs are left out and only counted
  (`secured_graphs_skipped`), never named. A graph whose policy changes while the query runs is left out too.
- Typed and untyped steps cross external links through the selected scope, including links stored in a third
  graph. Include that owner and both endpoint graphs in the scope. `r.graph` names the owner; `n.graph`
  names the node's graph. The same id in different graphs or named copies remains distinct.
  Custom edge fields are read from that owner too:

      umx query 'MATCH (a)-[r]->(b) RETURN r.confidence, r.why, b' --graph demo,ops,links
- An unavailable, hidden or broken far end gives no Cypher row, not a placeholder node.
  `external_edges_skipped` reports only the number of external links that could not be followed.
  Hidden graphs cannot be used as intermediate nodes. An unnamed secured owner contributes no link.
- `MATCH (g:graph) RETURN g` explicitly scans the scope's virtual graph nodes, one per selected graph.
  Their properties are `id`, `address` (`umx:<alias>`), `type` (`graph`), `title` and `alias`; other properties
  are null. Plain `(n)` scans, counts, search and similarity never include virtual nodes.
- A relationship pattern's start includes virtual nodes with visible incident edges, even without `:graph`.
  Thus `MATCH ()-[r]->() RETURN count(r)` counts links from virtual sources too.
  Binding happens at first appearance: `MATCH (n) MATCH (n)-[r]->()` first binds n from the standalone scan,
  so it excludes virtual sources. The same applies to comma-separated patterns.
  `count(DISTINCT n)` over `(n)-->()` may include virtual endpoints while `count(n)` over `(n)` does not.
  This is a deliberate umx-only departure; LadybugDB has no virtual graph nodes.
- Copies of one graph (the same graph, loaded under two aliases) are read once when no graph is named: the copy
  loaded first, with `copies_skipped` naming the others, so a count is never doubled. Naming copies
  (`--graph design,design_b`) reads each, with the warning `query_graph_twice`, and whole nodes and edges then say
  their `copy`. See `umx docs get graph`.

## Along a spine

`--along <spine>` with `--after <end>` and `--before <end>` runs the query on the nodes a spine places in that
range (both ends inclusive) and the edges between them: the induced subgraph. No path, fixed or variable-length,
passes through a node outside the range; an `OPTIONAL MATCH` to one gives null; an edge with either end outside is
not there; the graph node stays. One graph only. Label, relation and field warnings still read the whole graph, so
a label with no node in the range is not `query_unknown_label`. The reply's `along` and hints count the graph's
nodes left out (unlike node list's, which count the rows its filters kept; `umx docs get time`):

    umx query 'MATCH (d:decision)-[:raised_in]->(s:segment) RETURN d.title AS decision, s.title AS segment' --graph demo --along day --before 01JC8XB2K9

## Limits

- At most 1,000 rows come back; `--limit` takes up to 100,000, and `--offset` pages on. More rows than that
  gives `truncated: true` and the warning `query_truncated`.
- A work budget and a cap of 1,000,000 rows held at once stop a query that would explode
  (`query_too_large`, no rows), and a 10 s backstop stops a slow one (`query_timed_out`, no rows). Bind the start
  node by id, lower the bound, add LIMIT, or use `umx walk` or `umx path` for reachability.
- External steps consume the same work budget and cancellation checks as local steps.
- At most two queries run at once in the daemon; a third gets `query_busy` and nothing runs. Try again.

## Secret flags

A row names each column whose cell holds a value umx's secret check flags, as a read names a flagged field:
`"secret_suspected": ["field:t"]` on the row, and `secret_suspected` among the list's columns. A column is flagged
when anything inside its cell is: a node's title (by the read rule, so an allow clears it), a string (a list's items,
a map projection's keys and values), an edge's text-quote selectors, or the graph node's title. A stored node cell
with a flagged title also carries its own `"secret_suspected": ["field:title"]`, but the row decides: hide the whole
flagged column. `--human` prints each flagged column as `[secret suspected, hidden]`.

- A string column ignores allows: `RETURN n.title` stays flagged after `umx embeddings allow --node <n> --field
  title`, while `RETURN n` and `umx node get` do not. Read an allowed title with `umx node get`.
- The check sees the column's name, so `RETURN n.title AS password` can flag where a read would not.
- A column the query returns may not be named `secret_suspected` (`query_column_reserved`, nothing runs): an alias, a
  bare variable from WITH or UNWIND, or a variable `RETURN *` returns. Matched exactly, so `Secret_Suspected` runs. A
  umx-only departure: LadybugDB has no such rule.

## Refusals and what to write instead

| Code | Example | Write instead |
|---|---|---|
| `query_syntax` | `WHERE a.x != 1`, `` `name` ``, `RETURN 1 AS one` | the reason names the fix: `<>` for not-equal, snake_case names without backticks, start with MATCH; the position is the byte where reading stopped |
| `query_write_not_supported` | `CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE` | `umx node add`, `umx node edit` or `umx edge add` |
| `query_clause_not_supported` | `CALL`, `LOAD`, `USE`, `CREATE TABLE` | not part of the subset |
| `query_feature_not_supported` | `UNION`, `p = (a)-->(b)`, `shortestPath`, `a + 1`, `=~`, list comprehensions, `all(...)`, a variable on a variable-length step | `umx path <from> <to>` for paths, `size(...)` and `IN` for list tests |
| `query_ambiguous_labels` | `(n:question:criterion)` | `(n:question\|criterion)` |
| `query_unknown_function` | `type(n)`, `labels(n)`, `id(n)`, `type(r)` on a variable-length list, `now()`, `datetime(x)`, `datediff(...)` | `label(n)`, `n.id`, or `label(r)` on one edge per hop; relationship-list UNWIND is not supported; `current_timestamp()`, `timestamp(x)`, `date_diff(unit, a, b)` |
| `query_alias_shadows_variable` | `RETURN n.id AS n` | another column name, such as `n_id` |
| `query_order_by_entity` | `ORDER BY n` | order by a field, such as `n.id` |
| `query_with_order_needs_limit` | `WITH b ORDER BY b.id` | add `LIMIT 1000` (or SKIP) |
| `query_unbounded` | `-[:blocks*]->`, `-[:blocks*2..]->` | `-[:blocks*1..10]->` |
| `query_bound_too_large` | `-[:blocks*1..20]->`, `-[:similar_to*1..3]->` | an upper bound of at most 10, or 2 on a step naming similar_to |
| `query_symmetric_direction` | `(a)-[:conflicts_with]->(b)` | `(a)-[:conflicts_with]-(b) WHERE a.id < b.id` |
| `query_param_missing` | `$id` with no `--param id=...` | add the parameter |
| `query_unknown_variable` | `RETURN nope.id`, `count(zz)`, `label(r)` where `r` is not in the pattern | bind it in a pattern, UNWIND or WITH first |
| `query_column_reserved` | `RETURN n.title AS secret_suspected`, `RETURN *` over a variable of that name | another name, such as `AS secret_suspected_` |
| `query_invalid` | `UNWIND 1 AS x`, an aggregate in WHERE, two columns named alike | the reason says which rule |
| `query_too_large`, `query_timed_out` | an unbound walk or product | bind the start, lower the bound, add LIMIT |
| `query_busy` | a third query at once | try again in a moment |

Warnings never change the rows or turn the query into an error:

- `query_label_unused`, `query_relation_unused` and `query_property_unused`: an ontology in the selected
  scope declares the name, but no node or edge uses it yet. A property counts as declared only when a node
  type or relation declares that field. Declarations form a union across the graphs selected for this call.
- `query_unknown_label`, `query_unknown_relation` and `query_unknown_property`: the name is neither used nor
  declared in the selected scope. The query matched nothing or read null.
- Both categories point to `umx ontology show`. Do not widen scope to named or secured graphs just to
  remove a warning. A missing property on a virtual graph endpoint still reads null with its existing warning,
  even if a stored node elsewhere uses that field.
- `query_truncated` and `query_symmetric_in_directed_step` report the limits and directed-step cases above.

## Query or the other verbs

Use `umx neighbours`, `umx walk` and `umx path` for one node's edges, what it reaches, and a shortest path: they
are direct and need no query. Use `umx search` and `umx node find` to find nodes by words or title. Use
`umx query` when the question joins, counts or filters across edges.

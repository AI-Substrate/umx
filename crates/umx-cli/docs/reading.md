# Reading a graph

Which read to use for which question, how to read the answers, and the traps that make a graph seem to say
something it does not. The flags are in `umx docs get node`, `umx docs get explore`, `umx docs get search` and
`umx docs get query`.

## Orient first

    umx graph list
    umx graph guide <alias>      # what the graph is for, its words and what they mean

Read the guide of a graph you did not build before you answer from it: its conventions say what a status or a
relation means there.

## Which read

| Question | Read |
|---|---|
| A node you half remember by title | `umx node find "aproval delays"` (typos and word order are fine) |
| Which nodes mention something anywhere in their text | `umx search approval delay` |
| All nodes of a kind, filtered | `umx node list --type action --status open --where 'due_on<2026-12-01'` |
| One node in full: note, evidence, fields | `umx node get <node>`, then `umx node history <node>` for its status moves |
| What one node connects to | `umx neighbours <node>` |
| Everything upstream or downstream | `umx walk <node> --depth 3 --direction in --relation depends_on` |
| How two nodes are connected | `umx path <node> <node>` |
| What sits under a node by `part_of` | `--within <node>` on list, find, search and walk |
| Nodes like this one | `umx neighbours <node> --relation similar_to` |
| Joins, counts, absence ("questions with no answer") | `umx query '<cypher>'` |

Prefer the direct verbs: they need no query and say what they did. Use `umx query` when the question joins,
counts or filters across edges.

## Search

`umx search` fuses three legs into one list, best first:

- **exact**: the whole query equals a title, id, type, status or field value; exact hits come first;
- **words**: words in title, note and string fields, ranked by BM25: the word itself, its other forms (`vendor`
  finds `vendors`) and, from 4 letters, words that start with it (`vend`); each hit's `word_terms` says how;
- **meaning**: nodes whose vectors are close to the query's, when the graph has vectors.

Each row says how it was found (`legs`) and why (`cosine`, `bm25`, `coverage`), and carries `text`: up to 200
characters of the field that found it (`text_field`), graph data to judge the hit by before opening the node. `similarity_unavailable` means the
results are words and exact matches only. Try the other word form, or `umx node find` for titles.

## Similarity is a hint

`similar_to` edges and the vector leg say two texts are close, not that two nodes are the same thing or related.
Read both nodes before you act on it, and link them only with the relation you can justify.

## Traps

- **`open` may already be answered.** A question's status is only as current as its last edit. Before reporting a
  question as unanswered, look for incoming `answers` edges and for later nodes that address it:
  `umx neighbours <node> --direction in`, and search its words.
- **`answers` may be a promise.** An edge from "we'll get back to you" to the question is not an answer. Read the
  answering node's title and note before saying the question is closed.
- **`raised_by` is not ownership.** It says who said it. Who must act is `owns` (or an owner field). Don't report
  the person who raised an action as its owner.
- **A missing edge is not a no.** Graphs are built from what sources said. "Nothing blocks X" may mean no one
  mentioned a blocker.
- **A crossing needs all three graphs in the read.** Walk, path and query follow a link into another graph when the
  graph that stores it and both of its ends are in the read. With no graph selected (no `--graph`, `--scope` or
  `UMX_GRAPH`), the read is every loaded open graph plus any graph your addresses name; with `--graph a,b` or
  `--scope`, it is exactly those. `UMX_GRAPH` selects too: a walk from a bare id, or a query, then reads only that
  graph (an address in the command wins over it, and `umx query --all` ignores it). A named or secured graph is in
  the read only when you name it, with `--graph`, `--scope`, an address or `UMX_GRAPH`; so `UMX_GRAPH` set to a
  secured graph reads it on every bare read. A far end outside the read shows in walk and path as
  `graph_not_loaded`, and in query as no row plus `external_edges_skipped`; neither means the node is absent
  (`umx docs get linking`).
- **Secured graphs are skipped unless named.** `secured_graphs_skipped` in a reply means something was left out.

## Reading replies

In the common layouts a command about one thing puts it under its name (`data.node`), and a command that lists puts
rows in `data.rows`. On an error `data` is null, so check `status` first. `truncated: true` means more rows exist;
paging is per command (`--offset` on lists, walks and query; a larger `--limit` on find, history and search). A
`total` of null means unknown, never zero. The layouts, with jq, and the pattern for scripts under `set -e`:
`umx docs get output`.

## A handoff view

What someone picking up the work needs. Start with what the graph is for and what its words mean, then four reads:

    umx graph guide <alias>

    umx query 'MATCH (q:question) WHERE q.status = "open" AND NOT EXISTS { MATCH (q)<-[:answers]-() } RETURN q.id AS id, q.title AS title ORDER BY id'
    umx query 'MATCH (a:action) WHERE a.status <> "agreed" OPTIONAL MATCH (p)-[:owns]->(a) RETURN a.id AS id, a.title AS action, a.due_on AS due, p.title AS owner ORDER BY due'
    umx query 'MATCH (d:decision) RETURN d.id AS id, d.title AS decision, d.status AS status ORDER BY id'
    umx node list --where 'due_on<2026-12-01'

Check each open question against the traps above before handing it over.

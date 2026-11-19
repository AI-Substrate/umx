# Exploring a graph

Three read verbs follow edges across loaded graphs (req-21, req-28). A node is named as everywhere else: a full id, a
unique prefix of 6 or more characters, or an address `umx:<alias>/<id>`. Results are list rows, so `--human`
works as for `node list`; `neighbours` and `walk` also page with `--limit` and `--offset` (`path` returns one
whole path and takes neither). Edges are taken out before in, each in id order, so the same
graph always gives the same rows. `similar_to` edges are followed only when you ask for them with
`--relation similar_to`. A symmetric relation (`conflicts_with`, `alternative_to`, `possibly_same_as`, `same_as`, `not_same_as`, and any
marked `--symmetric`; see `umx docs get ontology`) is one edge both ways, so it is followed from either end
whatever `--direction` says; its `way` shows how it is stored.

`neighbours`, `walk`, `path` and `resolve` accept `--provenance` for this read only. Without it,
provenance nodes and their incident relations are absent, including as intermediate steps.
With it, traversal also follows the read-only `authored_by` and `edited_by` relations from a node to
its session, in either direction (`edge: null`, `derived: true`, no stored edge address). A session's
`session_of` relation leads to its agent. Derived attribution stays inside its graph and never comes
from an edge. `similar_to` never includes provenance, even with the flag.

The flag never widens graph selection. It shows provenance only in the graphs the read names (every graph read
when it names none): an external end's content shows from an open unnamed graph, but its provenance never does,
so name the far graph to see its provenance. Otherwise that link is omitted, without a placeholder or extra
skipped count. `umx who` and `umx node who` read attribution with no flag (`umx docs get who`).
An address of a hidden provenance node resolves as `node_not_found`; add the flag to resolve it.
Provenance is a record of what the writer claimed, never proof or permission.

## umx neighbours

Every edge at one node and the node at its other end. `--direction out|in|both` (default `both`),
`--relation <r>` for one relation. `--tag <a>[,<b>]` and `--no-tag <a>` (std graphs) list only the far ends whose
tags pass; see `umx docs get ontology`. `--entities on|off|confirmed` (default `on`) also lists the edges of the node's
entity members (below).

    umx neighbours umx:demo/01JC8XA4H7
    umx neighbours 01JC8XA4H7 --direction out --relation blocks
    umx neighbours 01JC8XA4H7 --tag needs_review

Rows: `way` (`out`: the edge leaves this node; `in`: it arrives), `relation`, `edge`, then the other node's `id`,
`type`, `title`, `status`.

### Entities: members' edges too

When the node is one entity with other nodes (a shared `entity_key`, a `same_as` link, or a `possibly_same_as`
proposal; see `umx docs get entity`), neighbours lists their edges as well, after the node's own. This is on by
default. `--entities off` lists only the node's own edges, as before; `--entities confirmed` leaves out proposals.
The entity is built at read time as `umx entity show` builds it, and nothing merged is stored.

    umx neighbours umx:crm/01JC8XA4H7
    umx neighbours umx:crm/01JC8XA4H7 --entities confirmed
    umx neighbours umx:crm/01JC8XA4H7 --entities off

A reply that groups adds `match_status` to every row (`own` for the node's own edges, `confirmed` for a member a
key or link joined, `proposed` for a `possibly_same_as` partner) and `via`, the address of the member an inherited
row came from (`umx:<alias>/<id>`); its `from` and `to` are the stored ends, so the member, not the node. A
proposal is a `possibly_same_as` partner of the node or of any confirmed member, as `umx entity show` lists it; it
brings only its own edges, never the partner's other members. Inherited rows are each member's
stored edges: an identity edge (`same_as`, `possibly_same_as`, `not_same_as`), an edge to another member, and
`similar_to` or `--provenance` rows are never inherited, and one stored edge is listed once. `--relation`,
`--direction` and `--tag` apply to inherited rows as to the node's own; `--limit` and `--offset` page the own rows
then the inherited ones. `--relation similar_to` never groups.

The list's `graphs` names the graphs the entity read covered: with no `--scope`, the node's graph and every loaded
open graph; a secured graph only when the read names it (by address, `--graph` or `--scope`), never as a member,
row or count otherwise, even when a link through it joins two open members. `--scope` pins the set. Two copies of
one graph never group. A reply that groups ends with the `entities_grouped` hint: how many nodes were grouped,
how many of them through proposals, and `umx entity show` to list the members. A reply that does not group keeps
today's rows; only `graphs` is added. When the entity cannot be read because loaded copies of a graph have no primary
(`copy_ambiguous` from `umx entity show`), neighbours gives the node's own rows as with `--entities off`; any other
refusal of the entity read is the reply's (an address in one graph with `--graph` naming another is
`graph_mismatch`). Under `--entities confirmed` a proposal partner is not a member, so a member's edge to it is an
ordinary inherited row (the `possibly_same_as` edge itself never is). Neighbours does not repeat `umx entity show`'s
warnings (two types holding a key, a link skipped because of a denial, a key held twice in a graph): the hint's
next, `umx entity show`, gives them.

Neighbours includes external links, including incoming links stored by other loaded graphs. Rows carry
`graph` (the other node's graph id), `edge_graph`, `from`, `to`, `from_graph` and `to_graph`. An unloaded or
unnamed secured end has only its stored ids and `state: "graph_not_loaded"`, never a title, type, status or
existence claim. A visible target missing the node is `edge_broken`. Links owned by an unnamed secured
graph are omitted. Use `umx edge list` with a scope to explicitly name secured owners or a target copy.
Visible loaded copies with no primary give `state: "copy_ambiguous"` and `candidates`, not
`graph_not_loaded`; name one of those copies in the edge-list scope. Secured unnamed copies are not candidates.

## Cross-graph reads

Walk, path and neighbours follow external links, including an edge stored by a third author graph.
`--graph demo,ops,links` or `--scope investigation` bounds the read; include the edge's owner as well as both
endpoint graphs. With no graph selection, open graphs are read once each, using their primary copy.
An address names its own graph and overrides an environment default. Name endpoints with addresses when
a bare id or prefix matches more than one node in the scope.

    umx walk umx:demo/01JC8XA4H7 --depth 3 --graph demo,ops,links
    umx path umx:demo/01JC8XA4H7 umx:ops/01JC8XB2KQ --scope investigation
    umx neighbours umx:demo/01JC8XA4H7 --scope investigation

Walk and path rows keep a bare `id`, adding `graph` (visible alias), `graph_id` and `address`. Walk also names
`from_graph` and `edge_graph`. Neighbour rows add `address` to their existing graph-id fields.
An unavailable endpoint is a terminal row with `state: "graph_not_loaded"`, `graph_id`, `node_id` and an
address, never a title, type or status. The address uses a known registry alias (`umx:<alias>/<node id>`),
or the graph id when no unambiguous alias is known. Both hidden and unloaded ends use that same registry
source, never the loaded graph's alias. Hidden secured graphs behave exactly like unloaded
ones, including when they are in the middle of a path. They are never traversed. A visible loaded end with
no such node is `edge_broken`; copies without a primary give `copy_ambiguous` and visible `candidates`.
Name a target copy in the scope to select it. A link owned by an unnamed secured graph is omitted entirely.

`umx:<alias>` denotes that graph's read-only virtual node (type `graph`, id equal to the graph id). It may be
an external edge endpoint and is not stored, listed or searched. Its fields are id, address, type, title and
alias, never a machine-local folder path. Use your hierarchy root for a same-graph edge instead.
`similar_to` stays local, must be named, and keeps its two-step depth cap.
Loaded graphs can also be named by id in addresses, including `umx:<graph id>/<node id>`.

## umx walk

Everything reachable from a node in 1 to `--depth` steps (default 1), breadth first. Each node is listed once,
at its shortest depth, with the node it was first reached `from` and the edge it came by. The start node is not
listed. `--direction out|in|both` (default `out`), `--relation <r>`, and `--within <node>` to list only nodes
under that node by `part_of` (the walk itself may pass through others). `--tag` and `--no-tag` (std graphs) work
the same way: only nodes whose tags pass are listed, and the walk goes on through the rest.

    umx walk 01JC8XA4H7 --depth 2
    umx walk 01JC8XA4H7 --depth 3 --tag 'origin:*' --no-tag blocked
    umx walk 01JC8XA4H7 --depth 3 --relation depends_on --direction in
    umx walk 01JC8XA4H7 --depth 2 --within 01JC8X9Q2M

Rows: `depth`, `id`, `type`, `title`, `status`, `from`, `relation`, `way`, `edge`.

A walk stops as soon as its page is full and it knows one more row exists, so a small `--limit` on a big graph is
cheap. Then `truncated` is true and `total` is null: the count is unknown without walking everything. Page on
with `--offset` (offset plus the rows you got). When `truncated` is false, `total` is the exact count.

    umx walk 01JC8XA4H7 --depth 5 --limit 20 --offset 20

## umx path

A shortest path between two nodes, at most `--max-depth` edges long (default 6), as the nodes in order with the
edge that led to each. `--direction both` (the default) lets the path follow edges either way; `out` keeps to
edge direction. `--relation <r>` for one relation. When several paths are equally short you get the first one
found (out before in, in id order). No path within the depth is an empty list, not an error.

    umx path 01JC8XA4H7 01JC8XB2KQ
    umx path umx:demo/01JC8XA4H7 umx:demo/01JC8XB2KQ --max-depth 3 --direction out

Rows: `step` (0 is the start), `id`, `type`, `title`, `status`, then for every step after the first the edge
taken: `relation`, `way` (`out` along the edge, `in` against it) and `edge`.

## umx resolve

What an address names in the loaded graphs. It takes an address `umx:<alias>/<id>`, where the id may be a
prefix of 6 or more characters, or an id or prefix in the graph the command selects (`--graph`, `UMX_GRAPH`,
or the only graph loaded). It only reads: it never loads a graph.

    umx resolve umx:demo/01JC8XA4
    umx resolve umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7#problem
    umx resolve 'umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7?repo=https://github.com/org/demo-graph.git'

When it resolves, `data.status` is `ok`, with `address` (a full id for stored nodes and edges), `graph`, `graph_id`, `kind`
(`node` or `edge`; a full edge id resolves to its edge) and the `node` or `edge` itself.
The whole-graph address `umx:<alias>` resolves to the read-only virtual graph node; its canonical address
stays `umx:<alias>`. Its only fields are `id`, `address`, `type`, `title` and `alias`.
The explicit `umx:<alias>/<graph id>` form resolves to the same virtual node.
An address that names a former alias of a renamed graph (`umx graph rename`) resolves with `status: redirected`,
`redirected_from` (the name given), `graph` and `address` under the live alias, and the hint `alias_redirected`.
Graph nodes do not accept fragments: `#<field>` and selectors on them are `usage_invalid` (`umx docs get explore`).
Read their five properties from the full result or `node get` instead. Explicit graph-id fragments are refused
once visible metadata identifies the virtual node.

- On a stored node or edge, `#<field>` (snake_case) gives `field` and its `value`. A field it does not have is
  `field_not_found`.
- `#seq=...`, `#L<n>...`, `#section=...`, `#quote=...` or `#page=...` is a selector (the evidence grammar, `umx docs get
  evidence`): it is returned as `selector` in canonical form (`#quote=we ship` comes back `quote=we%20ship`), with
  `read: false`. A selector or field evidence would refuse, resolve refuses too (`usage_invalid`, with the reason).
  umx does not read the passage yet.
- `?repo=<git url>` is only a hint and never changes what the address names. When the graph is not loaded,
  `graph_not_loaded` names the repo (if it is a plain `https://` or `git@host:path` URL) and `next` loads the graph
  once you have cloned it. umx never clones or fetches.

When it does not resolve, it exits 1 and the error says why: `graph_not_loaded`, `id_ambiguous` (a prefix
matching several nodes, listed as candidates; give more characters), `node_not_found`, `copy_ambiguous`
(no graph named, and copies of one graph with none primary; name one with `--graph`) or `field_not_found`.

A copy of a graph is addressed by the alias it was loaded under. `umx:<alias>@<copy>/<id>` is planned (req-69)
and is refused for now with `usage_invalid` (exit 2).

## Searching

`umx search <words>` is the text search. Exact matches (a title, id or field value equal to the query) come
first, then word matches (whole words in the title, the note and string fields, ranked by BM25) and meaning
matches (vectors close to the query's, when the graph has them) fused into one list. Each hit says which found it:
`legs`, `exact`, `word_rank` and `bm25`, `vector_rank` and `cosine`. A hint (`similarity_unavailable`,
`similarity_no_embedder`) or warning (`embedder_mismatch`) that meaning was not used still leaves correct word
results; with
`similarity_no_embedder` an empty result means no word matched. `--graph a,b` searches several graphs and
`--within <node>` a subtree; see `umx docs get search`.

To name one node by a half-remembered title, use `umx node find <words>`: approximate title match, typos and
word order allowed, best first, with `--type` and `--within <node>`.

    umx search approval delays --within 01JC8X9Q2M
    umx node find aproval delays --type problem

`umx node list --within <node>` lists everything under a node by `part_of`, with the usual filters.

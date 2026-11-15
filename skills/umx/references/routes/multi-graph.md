# Multi-graph scopes

Read `umx docs get scope` first (usage 100, superseding usage 098). Use `--graph a,b` for aliases or `--scope both`
for a saved scope, never both. `umx scope save both --graph a,b` stores graph ids; list/show/delete
manage the saved selection. Missing members refuse the whole read with a load next.
Saved hints name the primary's folder; every read reports moved folders, secured members and skipped copies.
`copy_dropped` applies only when saving a non-primary alias. Upgrade on `scope_format_newer`; never rewrite it.

Node and edge lists, query and search accept scopes. Default lists stay single-graph.
Default query/search omit secured graphs and extra copies; a saved scope explicitly names its secured members.
Use explicit graph aliases to read copies intentionally. **usage 118**: walk, neighbours and path also accept
scopes and cross external links, including third-owner links. Include the owner and both endpoints.
Unavailable ends stop at stored-id placeholders; hidden graphs are never intermediates. Writes still select
one author graph. `umx:<alias>` denotes that graph's read-only virtual node, never a stored root.

**usage 143**: on `copy_ambiguous` (copies of one graph, none primary), run its next, `umx graph primary <alias>`,
naming the copy unnamed reads should use; it survives a restart. Making a secured copy primary hides that graph id
from unnamed reads (`primary_is_secured`).

**usage 133**: one person, organisation or other thing in several graphs gets the same `entity_key` on each node
(`umx docs get entity`). Before adding it to another graph, look it up with
`umx node list --where entity_key=<key> --graph a,b` (or `--scope`), then copy the key onto the new node with
`umx entity key <node> add <key>`. `email:` and `upn:` keys are allowed and compared lowercase. A key needs
the same type to match. Fix a `duplicate_key` or a wrong match at its source: remove the key.

**usage 150**: when two existing nodes are one thing but share no key, `umx entity link <a> <b>` stores a
`same_as` edge (in `--graph`, else the first node's graph). `umx entity show <node>` lists the entity's members
by graph and how each matched (key or link), plus `possibly_same_as` proposals, over the graphs in `graphs`. Fix a
wrong match at its source with `umx entity unlink` or by removing the key.

**usage 172**: read `entity show`'s guard warnings (`entity_graph_twice`, `entity_key_ignored`,
`entity_types_mixed`, `entity_type_mismatch`) before trusting an entity; name it by the reply's `entity` or
`entity_id`; `umx node list --where entity_key=<key>` with no graph covers every loaded open graph.

**usage 173**: when a key or link wrongly joins two different things, `umx entity deny <a> <b> --reason <why>`
stores a `not_same_as` edge that beats keys and links (`entity_join_denied` names each skipped join);
`umx entity undeny <a> <b>` removes it.

**usage 186**: when one thing is an instance of another (a deployment of a product, a copy of a template),
`umx edge add <instance> instance_of <thing> --graph <graph>` relates them, across graphs if need be; it never
joins them into one entity. Use `same_as` only for one thing, and `part_of` for a thing inside another.

Choosing which graph a thing belongs in, or whether to make a new one: `umx docs get organise`. Linking to a
node that lives in another graph (when, when not, what reads follow the link): `umx docs get linking`.

**usage 193**: rename a graph with `umx graph rename <alias> <new>` and move its folder with `umx graph move <alias>
<folder>`, never by hand: old `umx:<old>/<id>` addresses keep resolving (`redirected`, hint `alias_redirected`;
write the new alias from then on), and a move lists the `root:repo` pointers to re-cite (`evidence_root_stale`).

**usage 139**: a `--scope` read loads members that unloaded when idle (each with the hint `graph_loaded_on_demand`); a member
you unloaded on purpose still refuses `scope_graph_not_loaded`: run its `next`.

# Scopes and saved graph sets

Use `--graph a,b` for a list of loaded graph aliases, or `--scope <name>` for one saved scope.
Do not use both. `--set` still sets node fields; it does not select graphs.
Give each once: a second `--graph` or `--scope` is refused (`usage_invalid`), never read as the last one given.

    umx node list --graph design,delivery
    umx edge list --graph design,delivery --type blocks
    umx query 'MATCH (n) RETURN n' --scope both
    umx search approval --scope both

Node and edge lists keep their single-graph default. With a list or saved scope, rows carry `graph` and
`address`, even with custom `--columns`. Lists concatenate graphs in scope order, each in id order;
`--limit` and `--offset` apply to the combined list, not separately to each graph.
Filters naming a node (`--within`, `--from`, `--to`) need an address when several graphs are selected;
they select matching rows in that address's graph, which must belong to the scope.

Query and search with nothing named still read all loaded open graphs, one primary per graph id.
There is no `--all` flag. Unnamed reads omit secured graphs with a count-only `secured_graphs_skipped`.
Named scopes count as explicitly naming their members, including secured graphs.

## umx scope save <name> --graph <graphs> [--replace]

    umx scope save both --graph design,delivery
    umx scope save both --graph design,delivery,decisions --replace

Names are snake_case in their own namespace: a scope and a graph may share a name.
An existing name gives `scope_exists` unless `--replace` is supplied.
Every member must be loaded when saved. Members are stored by graph id, with alias and folder hints,
in owner-only `$UMX_HOME/scopes.toml`. The daemon writes it atomically; scopes survive daemon restarts.
It is not a config layer, and repo config cannot define scopes.

Using a saved scope resolves each id to its loaded primary copy. Changing a loaded alias does not break it.
Copies are not saved: naming a non-primary alias warns `copy_dropped` and stores its primary's alias/folder
as the hints. Naming the primary does not warn. Reads report other loaded copies with `copies_skipped`;
secured copies are only counted. A saved scope is not a folder pin: if the primary now lives elsewhere,
`scope_member_moved` names the scope, saved member, saved folder and folder actually read.
A primary unloaded with several copies left gives `copy_ambiguous`.
`scope_includes_secured` warns at save and on every read that includes a secured member, including one
whose policy changed after saving; using that scope still explicitly includes it.

## umx scope list

    umx scope list

Lists saved names and their members.

## umx scope show <name>

    umx scope show both

Shows each member's saved alias, graph id, folder hint, loaded status and whether it is secured.
Security is `null` when the graph is not loaded, not an assurance that it is open.
A member that unloaded when idle, or did not come back after a daemon restart, is loaded for the read first, with
a `graph_loaded_on_demand` hint naming its folder, as for a graph you name with `--graph`. A member you unloaded
on purpose is not: the read is refused with `scope_graph_not_loaded` and a runnable `umx graph load <folder>` next,
and no other member is loaded for it. Supported paths containing spaces are quoted; unsafe shell punctuation
is still withheld (inspect the folder in scope show's data). It never silently returns a shortened scope.

## umx scope delete <name>

    umx scope delete both

Deletes the saved selection, not any graph. Missing saved names give `scope_unknown`.
Unreadable or untrusted storage is `scope_store_invalid`, a failed replacement is
`scope_store_not_saved`, and a scope store over 128 scopes, 128 members per scope or 60 KiB
is `scope_limit`. These failures do not fall back to all graphs or erase the previous store.
A newer `umx_scopes` version gives `scope_format_newer`, not an invalid-file error; upgrade umx.

## Single-graph operations

Writes, node get/find/history, ontology and check still take one graph. Neighbours, walk and path remain
single-graph until cross-graph traversal lands. They refuse `--scope` and a graph list with `usage_invalid`;
choose one alias with `--graph`. To read both copies intentionally, use their aliases in `--graph a,b`;
node/edge lists, query and search warn `query_graph_twice` and carry `copy` provenance.

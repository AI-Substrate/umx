# Linking across graphs

An edge can join a node in one graph to a node in another. This topic is about when to do that, when not to, how,
and what each read does with such links today. The command is in `umx docs get edge`; choosing graphs is in
`umx docs get organise`.

## When to link

Link when the thing you mean already lives in another graph, and that graph is where it belongs:

- a correction in a living graph pointing at the frozen record it corrects;
- a decision in your graph pointing at the requirement in the client's graph;
- an action in a team graph pointing at the project it serves in a programme graph.

Point at the canonical node: the one its owner keeps current. Don't copy the node into your graph to save a link;
the copy drifts from the original and no one knows which is right.

## When not to link

- **Not for likeness.** "These look alike" is a similarity, not a fact. `similar_to` edges and the vector leg of
  search are hints for you to check. Link only once you know how the two relate, with the relation that says so.
- **Not to avoid deciding.** If you are unsure which graph a thing belongs in, ask; don't put half in each and
  link them.
- **Not to a node you have not seen.** Find it first (`umx node find`, `umx search --graph <alias>`) and read it
  (`umx node get`). A link to an unloaded graph is stored unchecked.
- **Not to carry secured text.** An open graph may link into a secured one, but the link carries only the address.
  Never copy the secured node's title or text into the open graph.

## How

The edge is stored in one graph, the author; only the author's files change. Name the author with `--graph` and
give the other end as an address:

    umx edge add <from id> clarifies umx:workshop/<id> --graph engagement
    umx edge add umx:ops/<id> supports <to id> --graph engagement

- A loaded alias in an address is stored as its graph id, so the link survives a rename.
- For a target in a graph that is not loaded, give the graph id and the full node id:
  `umx:<graph id>/<node id>`. It is stored with `external_end_unchecked`: nothing checked the node exists.
- In `umx batch`, `from` and `to` take the same addresses; `_ref` fields still name nodes added in that batch.
- The first cross-graph link raises the author's file format; an older umx then refuses that graph until it is
  upgraded. Tell the graph's owner before the first one.
- `link_into_secured` warns that the target graph is secured: fine, as long as nothing secured is copied.

The other end's graph is not written, so you may link into a graph you do not own. Tell its owner if the link
matters to them.

## What reads do with links

- `umx edge list --from <node>` and `--to <node>` list links at either end, including links stored by other loaded
  graphs. Rows carry `from_graph` and `to_graph` (graph ids).
- `umx neighbours <node>` includes them. An end in an unloaded or unnamed secured graph shows only its ids and
  `state: "graph_not_loaded"`, never a title. Load and name it only when authorized; never infer absence from
  that state. A visible loaded end whose node is missing is `edge_broken`.
- `umx walk`, `umx path` and `umx query` follow external links. Bound the read with `--graph a,b,links` or
  `--scope name`, including both endpoint graphs and the edge's author. Hidden graphs cannot be intermediates.
  An unavailable end is terminal for traversal; Cypher returns no row and only a count warning.
- `umx check --graph <alias>` lists links whose target graph is not loaded (`graph_not_loaded`) and broken ones;
  `umx doctor` then reports the graph as degraded. Loading the target graph clears it.

`query --all` reads loaded open graphs even when `UMX_GRAPH` is set. Use `--include-named` only when the user
asked to read named graphs in this task, never as an automatic retry. Hidden-loaded and unloaded ends give
identical data and crossing diagnostics; only the separate `secured_graphs_skipped` aggregate count may differ.

## Whole-graph targets

Use `umx:<alias>` for a graph's read-only virtual node rather than inventing a stored root:

    umx edge add <from id> records umx:workshop --graph engagement
    umx node get umx:workshop
    umx resolve umx:workshop

Its id is the graph id; its properties are only `id`, `address`, `type`, `title` and `alias`. Ordinary node scans,
counts and search omit it. Cypher can bind it as an edge endpoint or with `MATCH (g:graph) RETURN g`.
For a link inside your own graph, use your hierarchy root instead of that graph's virtual node.

## Evidence that points at another graph

An evidence pointer can cite a node by address, with a quote from it:

    umx node note <node> --append "Per the workshop record" --evidence 'umx:workshop/<node id>#quote=approval'

That says where a claim came from. An edge says how two things relate. Use evidence for provenance and an edge for
a relationship you want walks and queries to see. Quote the words of the node the pointer names, not another's.

## Changing a link

The graph that stores a cross-graph edge can change it: `umx edge edit <edge> --relation <r>` or `--reverse`, and
`umx edge delete <edge>`, with the same rules as for a local edge (`umx docs get edge`). Name the edge by id in
the graph that stores it (`--graph` or its address). The far end is never loaded, read or named: it stays the
address the edge stores, and the reply shows that address, never the far node's title, whether or not that node
exists, is loaded or is in a secured graph. An edit never moves an end; to point the edge somewhere else, delete it
and add a new one.

## Planned

- Resource nodes for files and transcripts, and edges that point at lines in them (selector edges).
- A shared directory graph for people, so every graph links to one person node instead of keeping its own.

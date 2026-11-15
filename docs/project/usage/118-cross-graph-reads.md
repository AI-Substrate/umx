---
id: 118
title: Read links across graphs and link whole graphs
route: explore
status: current
source: ws005 s11, plan 090, 2026-11-11
reqs: [req-21, req-28]
supersedes: none
---
## Do

Use walk, neighbours and path across loaded external links. Bound a read with `--graph a,b,links` or
`--scope investigation`, including the edge's author graph and both endpoint graphs.
Use addresses when node ids or prefixes are ambiguous. Read `graph`, `graph_id` and `address` beside each
walk/path id. Cypher crosses the same links; `r.graph` is the author, not necessarily either endpoint.

Use `umx:<alias>` to link a whole graph through its virtual node. Read it with `umx node get umx:<alias>`;
resolve it with `umx resolve umx:<alias>` or query it explicitly with `MATCH (g:graph) RETURN g`.
Read its five properties from that result or `node get`, not from a fragment.

## Don't

Do not treat `graph_not_loaded` as proof that a secured graph is absent. Its stored-id placeholder is
deliberately identical to a hidden graph, and traversal stops there. Do not follow a hidden graph as an
intermediate or copy its text into an open graph. A hidden secured owner contributes no link.
Do not treat `copy_ambiguous` as unloaded: name a visible candidate copy in the scope.
Do not expect Cypher placeholders: unavailable external ends produce no row and a count-only warning.
With `--all`, hidden-loaded and unloaded ends give identical data and crossing diagnostics. D53's separate
`secured_graphs_skipped` aggregate count may differ; it names no graph.
Do not store a node of type `graph`, use a machine path as its address, or link your own graph's virtual
node. Use your hierarchy root for the last case. `similar_to` remains local and named-only.
On `node_id_is_graph_id`, a stored line uses the graph's reserved id. It and local edges naming it are kept
aside, never silently rewritten or revived. `umx check` lists the lines; edit the id by hand or add a new
node and re-point edges, then reload. The same id in a different graph is allowed.
Batch node adds explicitly using their own graph's id refuse with `id_taken`, even on an empty graph.
Graph fragments are refused: neither `umx:<alias>#field` nor a selector on a virtual graph node is accepted.
Resolve uses `usage_invalid` with `umx docs get explore`; evidence uses `evidence_pointer_invalid` with
`umx docs get evidence`. The explicit graph-id form has the same refusal once visible metadata identifies it.
A hidden or unloaded explicit-id evidence source remains `evidence_unchecked`; a citation never loads or
writes its target. This conservative boundary follows a project manager ruling after ws006 S2.

## Example

    umx edge add umx:demo/01JC8XA4H7 blocks umx:ops/01JC8XB2KQ --graph links
    umx walk umx:demo/01JC8XA4H7 --depth 3 --graph demo,ops,links
    umx path umx:demo/01JC8XA4H7 umx:ops/01JC8XB2KQ --scope investigation
    umx edge add 01JC8XA4H7 records umx:ops --graph demo
    umx node get umx:ops
    umx query 'MATCH (g:graph) RETURN g'

## Why

An author can connect graphs without modifying either endpoint graph. Scope, copies and policy still
apply at every crossing. Virtual graph nodes make whole graphs linkable without inventing stored roots.
This completes the traversal boundary deferred by usage 099; its write and security rules still apply.

Unavailable addresses use the alias in this machine's registry history, or a graph id if none is
unambiguous. They never use a hidden loaded graph's live alias. Loaded graph-id addresses resolve too.

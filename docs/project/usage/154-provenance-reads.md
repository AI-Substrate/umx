---
id: 154
title: Ask for provenance on the read that needs it
route: explore
status: current
source: ws007 s03, plan 131; architect, security, Cypher and addressing rulings, 2026-11-12
reqs: [req-95]
supersedes: none
---
## Do

Add `--provenance` to a graph read when you need session or agent records. Node get, list, find and
history, resolve, search, edge list, neighbours, walk, path, graph list/guide and query accept it.
Keep the graph scope explicit. Follow a node's `authored_by` or `edited_by` link to its session,
then `session_of` to its agent.

Search finds provenance through exact matches only; node find matches its titles. In Cypher, name
`authored_by` or `edited_by` on a single hop. Untyped Cypher steps never follow these derived relations.
Their id is null and they have no address. Default reads omit provenance from rows, counts and paths.

## Don't

Do not enable it as a graph, environment or configuration default, or pass it to writes. It never
widens graph visibility. Foreign provenance needs its graph in the read as well as the flag.
Never use recorded attribution as proof, permission or sign-off. Provenance is never embedded,
searched by BM25 or included in similar_to, even with the flag. For who made what, use `umx who` and
`umx node who`, which need no flag (usage 185).
`umx check` checks persisted records and retains provenance diagnostics without this flag.

## Example

    umx node list --type session --graph demo --provenance
    umx neighbours <node> --relation authored_by --graph demo --provenance
    umx query 'MATCH (n)-[r:edited_by]->(s:session) RETURN n.id AS node_id, s.id AS session_id, r' --graph demo --provenance

Without the flag, a reserved provenance name in Cypher warns `provenance_hidden` even if no
provenance exists. Ordinary user nodes of reserved types remain visible.

## Why

Provenance is bookkeeping, not subject matter. Leaving it out keeps everyday search, similarity,
counts and traversal unchanged. A per-read opt-in makes inspection possible without weakening
write, graph-policy or change-feed guards.

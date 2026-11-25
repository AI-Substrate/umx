---
id: 224
title: Find who cites a source with n.evidence_sources in one query across graphs
route: query
status: current
source: prime packet cypher-evidence-sources (2026-11-25; Jordan: fix it in the query language, not with a flag)
reqs: []
supersedes: none
---
## Do
Ask `umx query` which nodes and edges cite a source: `n.evidence_sources` and `r.evidence_sources` list each live
evidence pointer's source as written (no selector), in pointer order, and read null when there is none. Put the
source in `--param-str` and name the graphs (`--all`, `--graph a,b` or `--scope`). For every source under a prefix,
UNWIND the list and test each item with `STARTS WITH`. For a file cited through its resource node (`umx:<g>/<id>`),
add a second hop through the resource's `locator_source`.

## Don't
Don't grep nodes.jsonl or edges.jsonl for a source. Don't write `any(s IN ...)`: list predicates are refused
(`query_feature_not_supported`). Don't expect removed or redacted pointers, or a status move's evidence, in the list.
Don't `--set evidence_sources`: the name is reserved. A record that already holds its own field of that name loads
with `evidence_sources_field_kept`; remove it with `umx node edit <id> --unset evidence_sources` (or `umx edge edit`).

## Example
```bash
umx query "MATCH (n) WHERE \$src IN n.evidence_sources RETURN n.graph AS g, n.id AS id, n.title AS title" --param-str src=calls/3-nov.vtt --all
umx query "MATCH ()-[r]->() WHERE \$src IN r.evidence_sources RETURN r.graph AS g, r.id AS id" --param-str src=calls/3-nov.vtt --all
umx query "MATCH (n) UNWIND n.evidence_sources AS s WITH n, s WHERE s STARTS WITH 'root:repo/FY27/' RETURN DISTINCT n.graph AS g, n.id AS id, n.title AS title" --all
umx query "MATCH (f) WHERE f.locator_source = 'calls/3-nov.vtt' MATCH (n) WHERE f.address IN n.evidence_sources RETURN n.graph AS g, n.id AS id, n.title AS title" --all
```

## Why
An agent needed every claim one transcript supports and had to grep eleven graphs' files. The answer is a read of
data umx already holds, so it belongs in the query language. Sources match as written, and the two-hop form matches
the address only as umx stores it (`umx:<alias>/<full id>`). A source that looks like a secret flags its column, as
any query string does.

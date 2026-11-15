---
id: 099
title: Store external edge ends in one author graph
route: build
status: current
source: ws005 s07b-2 plan 068, architect and security decisions 2026-11-11
reqs: [req-21, req-23]
supersedes: none
---
## Do

Name the author with `--graph <alias>` when either end belongs to another graph. Either or both endpoints
may be `umx:<graph>/<id>`; a loaded alias is stored as its graph id. Only the author's files change.
For an unloaded target use a graph id and a complete node id. `external_end_unchecked` means the target
was not checked. An open-to-secured link warns `link_into_secured` and carries only the supplied address.

Use the same addresses in batch `from` and `to`; `_ref` fields remain local. Read external links with
`edge list --from/--to` or `neighbours`. Each end carries its graph id. An unloaded or unnamed secured
target shows ids only and `graph_not_loaded`; a visible missing node is `edge_broken`, also in `umx check`.
An incoming link owned by an unnamed secured graph is omitted entirely.
An environment default overridden by `--graph` or an address does not authorize that default graph.

Loaded copies without a primary are `copy_ambiguous`, not unchecked. For a write name a copy in the
endpoint address; for a read name it in the scope. Read candidates name visible copies only. Declaring a
relation symmetric collapses local and external reverse pairs at once, keeps their normalized lines and
unknown fields, and updates the incoming index only after commit.

## Don't

Do not guess a graph id from an unloaded alias, set graph keys with `--set`, copy secured content into
an open graph, or treat an unchecked link as proof that its node exists. Walk, path and Cypher crossing
are not implemented yet. Do not assume an older umx can load an author after its first external link:
that write atomically raises the file format to 3, and the version never drops.

## Example

    umx edge add 01JC8XA4H7 blocks umx:ops/01JC8XB2KQ --graph launch
    umx edge list --from 01JC8XA4H7 --graph launch
    umx neighbours 01JC8XA4H7 --graph launch

For a hand-edited own-graph key, `graph load` warns `own_graph_end`, treats the end as local and removes
the redundant key on the next write, while preserving the record and unknown fields.

## Why

One graph owns the edge and its atomic write. Stored graph ids survive alias changes and copies.
Read-time policy checks keep secured targets and secured edge owners from leaking through links.

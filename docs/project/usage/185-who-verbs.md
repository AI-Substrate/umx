---
id: 185
title: Ask who made what with umx who and umx node who
route: explore
status: current
source: ws007 s04, plan 188; security, architect, graph-addressing and graph-storage rulings on the design note, 2026-11-13
reqs: [req-95, req-96]
supersedes: none
---
## Do

Use `umx node who <node>` to see who made a node and who last edited it: the session, agent, role, model and
harness, each with its address in the node's graph. Use `umx who <key|seat|session>` to list the nodes and edges
an agent or session made, last edited, or added or removed evidence on, across the graphs the read covers.
Neither needs `--provenance`.

Give `umx who` a full key (`agent/pij/<seat>`, `session/<harness>/h-<16 hex>`) or a bare seat, user or stored
session id. A bare id must end exactly one agent or session key in the graphs read; on `who_ambiguous`, run it
again with one key from `candidates`. A cli user is always ambiguous (an agent and a session share the name), so
give `agent/cli/<user>` or `session/cli/<user>`. Narrow with `--graph a,b` or `--scope`, and page with `--limit`
(read `total` and `truncated`).

The addresses node who prints name provenance records: read them with `--provenance` on that read
(`umx node get <address> --provenance`). Provenance shows only in the graphs a read names (every graph read when it
names none): name a far graph to see its provenance.

## Don't

Do not pass `--provenance` to who or node who: they refuse it. Do not give a raw harness session id: umx stores only
its hash and never looks one up. Do not treat an answer as proof, permission or sign-off: provenance is recorded by
the writer, not verified. Do not expect a secured graph to answer unless you name it: with no graph named, secured
graphs are left out and only counted (`secured_graphs_skipped`), so an answer never says where a seat wrote in
them.

## Example

    umx who pij-example-seat
    umx who agent/pij/pij-example-seat --graph alpha,beta --limit 20
    umx node who umx:alpha/01JC8X

## Why

Agents need to know which session made a change, to ask it or to follow its trail, without turning provenance on
for every read. Joining graphs on the key, never on node ids, gives one answer for a seat across graphs while each
graph keeps its own records and its own policy.

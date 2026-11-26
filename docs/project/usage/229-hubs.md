---
id: 229
title: Read a graph's hubs before adding nodes, put each node in the hubs it belongs to, and set hubs up with umx hub
route: build
status: current
source: PRD 003 graph rules, hubs (rules-15 to rules-21, rules-31 to rules-34), 2026-11-26
reqs: []
supersedes: none
---
## Do
Read `umx docs get hubs`. Before adding nodes, run `umx hub list` to learn where things gather
(a risk register, each meeting). Put a node in a hub with the ordinary edge `hub list` prints:
`umx edge add <node> part_of <hub>` for a named hub, `--raised-in <meeting>` on the add or batch
for a meeting. Set a named hub up with `umx hub set <name> --node <id>` and a pattern
(`--type risk`), so umx adds matching nodes itself; set meetings up once with
`umx hub set meetings --type-hub meeting --via raised_in`. Read `hub_joined` on an add or edit:
umx put the node in that hub in the same write and named the edge under `hub_edges`. After a
`hub_scan_cut`, run its next (`umx hub fill <name> --after <node>`) until it no longer warns.
`umx hub show <name>` lists members and matching nodes that are not members; `umx hub fill <name>`
joins those.

## Don't
Do not look for a `umx hub add`: adding by hand is the edge. Do not set `auto_added_by` yourself;
it is refused. Do not give a type hub a pattern: umx cannot know which meeting a node belongs to,
so the writer links it. Do not expect umx to remove an edge it added, or to add one on a read,
load, check or edge write: a hand-deleted auto edge stays deleted until `hub fill`. Do not set a
hub when older umx must still read the graph: the first hub raises graph.json to format 9.

## Example
```bash
umx hub set risk_register --node 01JZ0000000000000000000001 --type risk --graph demo
umx hub set meetings --type-hub meeting --via raised_in --graph demo
umx node add --type risk --title "Supplier may slip" --raised-in 01JZ0000000000000000000002 --graph demo
umx hub show risk_register --graph demo
umx edge list --where auto_added_by=risk_register --graph demo
```

## Why
Hubs tell an agent where a node belongs before it writes, so the graph is well structured and
easy to walk later. They nudge and never fence: a hub never warns about a node's shape.

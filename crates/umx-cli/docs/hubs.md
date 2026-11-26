# Hubs: where nodes gather

A hub is an ordinary node that other nodes gather around. Its members are the nodes with an
edge of one named relation to it: the member is the edge's `from`, the hub is its `to`. Hubs tell
an agent where things belong (a new risk goes in the risk register) so the graph stays easy to
walk. A hub is not a rule: it never warns about a node's shape.

A graph declares its hubs in graph.json, each with a name, in one of two forms:

- **Named hub**: one node is the hub. Members join it by `part_of` unless `--via` names another
  relation. It may carry a pattern; umx then adds matching nodes to it itself.
- **Type hub**: every node of one type is a hub, as every `meeting` is a hub for the nodes
  `raised_in` it. `--via` is required. It takes no pattern: with many hubs of one type, umx
  cannot know which one a node belongs to, so the writer links it.

A graph may have many hubs, many of one type, and a node may be in several at once.

## Set up a risk register and meeting hubs

    umx node add --type topic --title "Risk register" --graph demo
    umx hub set risk_register --node <register id> --type risk --graph demo
    umx hub set meetings --type-hub meeting --via raised_in --graph demo
    umx hub list --graph demo

`hub set` with a pattern joins every matching node that is not yet a member, in one write, and
lists the nodes joined with a total. Setting the same declaration again writes nothing and says
`unchanged`. Setting a different declaration under the same name replaces it; the edges umx added
for the old one stay, and `hub_edges_remain` says how many and how to list them. Hub names are
snake_case; a graph holds at most 64 hubs. The first hub raises graph.json to format 9; a graph
with no hubs keeps its format and bytes.

Patterns take the same flags as a rule's (`umx docs get rules`): `--type`, `--status`, `--tag`,
`--where <field>=<value>` and `--match <field>=<regex>`. A type hub with a pattern is refused
with `hub_invalid`. Provenance records and the hub node itself are never joined.

## Add a member by hand

Adding to a hub is one ordinary edge of the hub's relation. `hub list` and `hub show` print the
exact command for each hub:

    umx edge add <node> part_of <register id> --graph demo
    umx node add --type decision --title "Ship on Friday" --raised-in <meeting id> --graph demo

A hand-added edge carries no `auto_added_by`.

## What umx adds by itself

umx adds a node to a named hub with a pattern at three moments, and no others:

- `umx hub set` (the backfill above) and `umx hub fill <name>`;
- `umx node add` and `umx batch` node adds, when the new node matches;
- `umx node edit`, `umx node status`, a `node add --on-key resolve` that changes the node it finds, and their batch
  lines, when the change makes the node match (it did not before).

The edge is added in the same write as the node, and the graph's rules check it as they check an edge add. The reply says so with the hint `hub_joined`,
naming the hub, with `umx hub show <name>` as its next; its data names each edge under
`hub_edges`. Each such edge carries the field `auto_added_by: <hub name>`. Only umx sets it: an
`edge add --set auto_added_by=...` or `edge edit --set auto_added_by=...` is refused with
`reserved_name`. Its other fields stay editable.

    umx edge list --where auto_added_by=risk_register --graph demo

umx never removes an edge it added: not when the hub is removed or set again, not when a node is
retyped out of the pattern. It never adds one on a read, a load, `umx check` or an edge write, so
an auto-added edge you delete stays deleted, and a matching node that arrives by a git merge is
not joined until you ask. `hub show` lists matching nodes that are not members, with `hub_unjoined`
and `umx hub fill <name>` as its next.

## Large graphs

`hub set` and `hub fill` read nodes in id order and stop at a scan limit (16 MiB of node lines,
1,000 joins or the pattern's work budget) rather than refuse. The nodes reached are joined in
that one write; `hub_scan_cut` names where it stopped, and its next carries on:

    umx hub fill risk_register --after <node id> --graph demo

A member is skipped, so fill is safe to repeat. A node too long for a regex to finish is listed in
`not_checked` (the first 100, with `not_checked_total`) and passed over.

## Read hubs

    umx hub list --graph demo
    umx hub show meetings --graph demo --limit 20 --offset 0

`hub list` gives each declaration (`name`, `form`, `via`, `node` or `type`, any `match`), how many
hub nodes and members it has, and `add`, the command that adds a member by hand. `hub show` pages
the hub nodes (`--limit` 1 to 1000), each with its members, and for a named hub with a pattern the
matching nodes that are not members. `umx ontology show` carries the declarations under `hubs`
too, which is what the graph view draws declared hubs from.

## Meetings

When nodes come from a meeting: find its one `meeting` node first (`umx node find "<title>" --type meeting`) and
add it only if missing, with `starts_on`. Declare the meetings hub once (above) and the meeting spine once:

    umx graph spine set demo --file meetings-spine.json

with `{"name": "meetings", "title": "Meetings", "main": true, "anchors": {"type": "meeting"}, "order":
["starts_on"], "place": ["raised_in"]}` in the file (`"main": true` only if the graph has no main spine yet; a second one is refused with `spine_main_taken`, so use `false` then). Then link every point with `--raised-in <meeting>`. The
optional `from_a_meeting` rule warns about a point with no meeting (`umx docs get rules`).

## Hubs and spines

A hub is an ordinary node, so a spine places it as it places any node: meetings sit on a meeting
time spine; a risk register sits on none. Members are placed in their own right. An edge umx added
for a hub is never read by a spine as a node's parent or place, so joining a hub never moves a node
on a spine.

## Removing

    umx hub remove risk_register --graph demo

Removing a hub drops its declaration; its auto-added edges stay, and `hub_edges_remain` gives the
`edge list --where` that lists them.

umx has no node delete: a node that no longer holds is retired to `ruled_out` or `superseded`. Retiring a
named hub's node warns `hub_node_retired` at the move, and on load, `umx check`, `hub list` and `hub show`,
with `umx hub remove <name>` as its next; the hub still gathers members until you remove it. A named hub
whose node is no longer in the graph at all (a hand edit or a merge) adds nothing; `hub_node_missing` says
so on load, `umx check`, `hub list`, `hub show` and `hub fill`. The graph always loads.

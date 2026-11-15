---
id: 170
title: Ask umx which spines a graph could have, and clear what is in the way
route: build
status: current
source: time review B4, B4a and B5 (an internal time review, 2026-11-12); ws005 s07, plan 156
reqs: []
supersedes: none
---
## Do

Once a graph has its anchors (meetings, segments, phases, each with its date or `step`, and points linked to
them), run `umx graph spine propose <alias>`. Read the offered candidates first: each has its cover (how many
nodes it places by their own field, by a link, by evidence position, and how many it leaves out) and its spread.
Fix each `in_the_way` finding with its `next`: make steps held as field values into nodes, settle step numbers
that disagree with `precedes` (per `part_of` parent; two siblings with the same step are fine), and check points
linked to one segment whose evidence sits in another. List the nodes the best spine leaves out with
`--unplaced <candidate>`, exactly as the candidate is named (`type:segment`, `date_field:due_on`; 100 ids a page
unless you give `--limit`), and link them.
Write the spine you chose in the graph's conventions.

## Don't

Don't take `recorded` as the graph's time: it is when the graph was written, and it is not offered when every node
was written in one sitting (`no_spread`). Don't expect a link into another graph to place a node, or a `umx:`
evidence source to be compared. Don't treat a missing spine as an error: `umx graph guide` and `umx check` show it
as a hint and `umx check`'s exit code does not change. Proposing stores nothing; confirming comes with a later umx.

## Example

    umx graph spine propose demo
    umx graph spine propose demo --unplaced type:segment --limit 50

## Why

A graph has several orders (the day as it ran, the method, the source order), and only the builder knows which
one they meant. umx can see which orders the data supports, how much of the graph each one places, and what stops
the rest, so the builder confirms a spine from evidence instead of guessing. A number an agent can move is worth
more than a paragraph of advice.

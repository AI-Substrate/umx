---
id: 179
title: Link what a window adds to where it came up with --raised-in
route: ingest
status: current
source: ws008 TM4, plan 177 (the project manager 2026-11-13; D71; the time review's "a default anchor for a batch")
reqs: [req-148]
supersedes: none
---
## Do
When you ingest a transcript in windows, make and date the segment (or the meeting) first, then run each window's batch with `--raised-in <segment>`: every node the batch adds gets a `raised_in` edge to it in the same write, and each row names the edge (`raised_in_edge`) and the anchor (`raised_in`). A node that came up somewhere else takes its own `"raised_in":"<anchor>"` on its `node add` line. For a single add, use `umx node add ... --raised-in <anchor>`. The anchor may be in another graph (`umx:<graph>/<id>`). Dry-run first: the rows name each anchor.

## Don't
Don't write a `raised_in` edge line for every node a window adds. Don't point `--raised-in` at a node of a type raised_in does not expect (a person, a risk): it warns `relation_endpoint_unexpected`. Don't expect edits or edge lines in the batch to get the edge: only the nodes it adds do.

## Example
```bash
umx batch --file window-07.jsonl --graph launch --raised-in umx:launch/01JC8XA4H7 --dry-run
umx batch --file window-07.jsonl --graph launch --raised-in umx:launch/01JC8XA4H7
umx node add --graph launch --type question --title "Who signs off?" --raised-in umx:launch/01JC8XA4H7
```

## Why
A time spine places a point by the anchor it came up in (D71), and linking 200 nodes by hand is the step agents skip. One flag per window makes the link free, and the anchor resolves as an `edge add` end does, so a wrong anchor fails before anything is written.

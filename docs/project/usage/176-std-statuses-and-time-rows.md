---
id: 176
title: Use the std lifecycle statuses and the std time names
route: build
status: current
source: ws008 T1, plan 171 (the project manager 2026-11-13; D71 R1 and R5; graph-addressing-expert topic:ws008-std-statuses and topic:ws008-time-std)
reqs: [req-146, req-98]
supersedes: none
---
## Do
For work that moves (an engagement, a project, an action), use the four std lifecycle statuses on any type that fits: `active` (under way), `stalled` (started, not moving, not given up), `delivered` (done, outcome handed over) and `withdrawn` (given up or cancelled). Until your graph opts in to std, declare the ones you use with `umx ontology add status <name>`. Move between them freely, and say why for `stalled` and `withdrawn`: `umx node status <node> stalled --reason "<why>"`. Keep a free-text outcome in the reason or a note.

Use the std time names: a part of a meeting or event is a `segment`, `part_of` its meeting, event or a larger segment, with `starts_on`, `ends_on` and a `step` (an int from 1, counted among the siblings under one parent). `starts_on` and `ends_on` are the one span pair on any node or edge. Order things of one type with no clock with `precedes`; link a point to the finest anchor it came up in with `raised_in` (several are fine).

## Don't
Don't invent synonyms: `delivered` is not `done`, and there is no `complete` or `cancelled`. Don't replace the built-in `open`. Don't put an outcome or a decision date in a field: when something was said comes from `raised_in` and the status move. Don't compare `step` values across parents, and don't write `valid_from` or `valid_to`.

## Example
```bash
umx ontology add status stalled --graph launch
umx node status umx:launch/01JC8XA4H7 stalled --reason "waiting on the vendor's contract"
umx node add --graph launch --type segment --title "Morning session" --parent umx:launch/01JC8XB2K9 --set step=1 --set starts_on=2026-11-13T09:00:00+10:00
umx edge add umx:launch/01JC8XC3M1 raised_in umx:launch/01JC8XD4N2
```

## Why
Graphs that name the same things the same way line up: a standing (unsettled, settled, out of play) means the same across graphs only when they share the status names, and one spine recipe fits every graph that dates its segments and links points with `raised_in` (D71 R5). These names are std v1, the table built into umx; graphs opting in to it comes later, so declare them for now.

---
id: 187
title: Declare a graph's spine with graph spine confirm or set, and read it with show
route: build
status: current
source: ws008 TM5, plan 194 (the project manager 2026-11-13; D71; the time review's step 9; architect's format ruling)
reqs: [req-149]
supersedes: none
---
## Do
Once the anchors are dated and the rest linked, run `umx graph spine propose <alias>` and declare the spine you choose. `umx graph spine confirm <alias> type:<t>` stores a proposed candidate's declaration unchanged. To make one the graph's `main` spine, or to change its order or place, write the declaration (the `decl` shape propose lists) to a file and run `umx graph spine set <alias> --file <decl.json>`; the same name replaces it. `umx graph spine show <alias>` places the graph on each declared spine as it is now: its cover (`by_field`, `by_link`, `by_evidence`, `by_rule`, `not_placed`) and spread. Fix what is not placed by linking it to its anchor, then show again. On a spine ordered by a date, a node that holds that date itself now sits in the band of the last anchor dated at or before it, and counts toward the spread (D71 step 12, usage 192); while no anchor holds that date, a node's own date places nothing; `umx graph spine show <alias> --unplaced <name>` lists the nodes the spine does not place. List the nodes placed only by rule (they sit beside what they link to, not on data) with `umx graph spine show <alias> --by-rule <name>` (or `propose --by-rule <candidate>`; each reply names that command) and link them to their anchor.

## Don't
Don't name a type, relation or field the graph has not declared and std does not have: it is refused `spine_name_unknown`, whose next is the exact `umx ontology add` command; run it, then confirm or set again. Don't declare two main spines (`spine_main_taken`). Don't confirm a built-in candidate (`recorded`, `status`, `date_field:<f>`, `source`): every graph has those (`spine_builtin_candidate`). Don't read `by_rule` as placed by data: those nodes only sit beside what they link to. Don't declare a spine in a graph an older umx must still open: the first spine moves graph.json to format 6, which an older umx refuses.

## Example
```bash
umx graph spine propose demo
umx graph spine confirm demo type:segment
umx graph spine set demo --file day-spine.json
umx graph spine show demo
umx graph spine remove demo segment
```

## Why
A declared spine says how the graph is ordered, in the graph itself, so every reader (the CLI and the view) lays it out the same way and reports the same cover (D71, time review step 9). The rule placement is fixed in umx with named settings, never an expression, so the answer does not depend on who asks.

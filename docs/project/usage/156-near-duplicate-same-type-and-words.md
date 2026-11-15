---
id: 156
title: near_duplicate compares a node with its own type, and the words that differ must be a typo
route: build
status: current
source: ws004 s30 (plan 132), user report 2026-11-12; design agreed with the embeddings-expert and the graph-storage-expert
reqs: [req-57, req-135]
supersedes: none
---
## Do

Treat `near_duplicate` as "this may be the same thing": check the candidate with the hint's `next` and reuse it
unless it is genuinely different. The warning now names the candidate's type ("A component node with a similar
title exists."). A node of the same type warns from a title score of 0.85; a node of another type only from 0.95,
when the wording is near identical.

In `umx batch`, when the closest title is a node an earlier line of the same batch adds, the warning is
`near_duplicate_line` and names that line ("Line 7 of this batch adds a risk node with a similar title."). If the
two lines are the same thing, keep one. Its `next` is `umx node get` on the node after a real run, and the batch
dry run (`umx batch --file <file> --dry-run`) in a dry run or failed batch, where that node has no id.

## Don't

Don't expect a warning when two titles share a scaffold but differ in a real word: "Acme Ops Data Science team"
and "Acme Ops Geoscience Data team" are not near duplicates. After the shared words are taken away, the words left
must be within a typo of each other (about one edit per four letters). Joined or split words ("work flow",
"workflow"), plurals and misspellings still warn.

Don't expect a warning between titles that differ in a number: "Phase 1 rollout plan" and "Phase 2 rollout plan",
"v2" and "v3", "2025" and "2026", or a title and the same title plus "v2" are different things, not copies.
The same holds for a short token (under four letters) such as "Unit A" and "Unit B", even when another word in the
titles also has a typo.

Don't rely on `near_duplicate` across types for anything but near-identical wording, and don't rely on it for the
same meaning in other words: titles are compared by spelling only.

## Example

Two separate cases, each in a graph holding one component titled "Pricing engine cache layer":

    umx node add --type component --title "Pricing engine cache layers"
    # hint near_duplicate: A component node with a similar title exists.
    #   next: umx node get umx:demo/<the existing node's id>

    umx node add --type limitation --title "Pricing engine cache layers"
    # no near_duplicate: the component is another type and scores 0.945, under 0.95

## Why

Field use on testers' graphs found near_duplicate firing on a limitation titled like a component, on team
names that share an organisation and unit but differ in the team's subject (0.93), and, in a batch, telling the
agent to run `umx node get <graph>/<id>` for a node an earlier line had not yet written. Whole-title character
similarity rewards long shared prefixes and suffixes, so it needs the words that differ to be close too.

---
id: 165
title: Write dates with their offset, date the anchors, and link the rest
route: build
status: current
source: Time decisions R1, R4 and R5 (2026-11-12, the internal time review 6.1 and 7 step 2); ws008 TM1, plan 146
reqs: [req-144]
supersedes: none
---
## Do

Declare date fields as kind `date` before using them (`umx ontology add field meeting starts_on:date`). Write a
date-time with its offset or `Z` (`2026-10-30T09:40:00+11:00`): umx stores it in universal time
(`2026-10-29T22:40:00Z`). Use `starts_on` and `ends_on` for any node's or edge's span in the world. Make the
meeting a node with full date-times, a long meeting's parts `segment` nodes `part_of` it with a `step`, and
link each point to where it came up with `raised_in`. For a month or year, write it as it is (`2025-01`, `2025`;
usage 161). To ask about a local day, compare with date-times, not a plain date.

## Don't

Don't write a date-time without an offset: umx takes it as universal time (it warns `time_no_offset`, usage 178). Don't use
`valid_from` or `valid_to`, put a time in a string field, date every node, guess a date, or read a time from an
id or from `created`. Don't treat when something was said as a date field: it is a `raised_in` link. Don't
invent a first day or add `date_precision` for a month or a year (usage 161).

## Example

    umx ontology add field meeting starts_on:date
    umx node add --type meeting --title "Friday session" --set starts_on=2026-10-30T09:40:00+11:00
    umx node list --type meeting --where 'starts_on>=2026-10-30T00:00:00+11:00' --where 'starts_on<2026-10-31T00:00:00+11:00'

`umx node list --where 'starts_on<2026-10-30'` includes that Friday morning meeting: it is
`2026-10-29T22:40:00Z`, and a plain date is a universal-time day.

## Why

One stored format keeps every instant and needs no zone on disk (R4); a view converts to local time. One span
pair (R1) leaves agents no wrong choice. Time stored once on a few anchors and inherited through links stays
consistent, and it is what a timeline needs to place the rest (R5).

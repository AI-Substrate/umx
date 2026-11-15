---
id: 161
title: Write a month or a year as it is, and dates compare as spans of UTC time
route: build
status: current
source: plan 147 (ws008 TM2), time-dimension review R4 and R5, 2026-11-12; F124
reqs: [req-145]
supersedes: none
---
## Do

When a source gives only a month or a year, write exactly that in a `date` field: `--set due_on=2025-01` or
`--set starts_on=2025`. umx stores it as written. Write a date-time with its offset
(`--set starts_on=2026-10-30T09:40:00+11:00`); umx stores it in UTC (`2026-10-29T22:40:00Z`).

Read a date as a span of UTC time: a year, a month or a day is that whole UTC period, a date-time its one second.
`--where` and `umx query` compare two dates by those spans, with one rule for both:

- `<` wholly before: `due_on<2025-02` is anything that ends by 1 February 2025.
- `>` wholly after.
- `=` overlapping: `due_on=2025-01` finds `2025-01-15` and `2025-01-31T23:00:00Z`, and also `2025` (it holds
  January); `!=` is not overlapping.
- `<=` and `>=` are either.

Sorting (`ORDER BY`, `min`, `max`) goes by start, then the longer span first: `2025`, `2025-01`, `2025-01-01`,
`2025-01-01T00:00:00Z`. Any text that reads as a date compares as a date, also in a string field.

## Don't

Do not invent a day (`2025-01-01`) for a month, and do not add a `date_precision` field: the value carries its
precision. Do not write a date-time without an offset unless it is UTC: umx takes it as UTC, and local 09:40 typed
bare is stored hours out. Do not read a UTC day as the local day: a Friday 09:40 meeting in Sydney is
`2026-10-29T22:40:00Z`, before `2026-10-30` and on `2026-10-29`; that is correct. In `umx query`, write a year as
text (`"2025"`, `--param-str y=2025`); the number `2025` is never a date.

## Example

    umx ontology add field action due_on:date
    umx node add --type action --title "Send the budget" --set due_on=2025-01
    umx node list --type action --where 'due_on<2025-02'      # wholly before February
    umx node list --type action --where 'due_on=2025-01'      # anywhere in January
    umx query 'MATCH (m:meeting) WHERE m.starts_on = "2026-10-29" RETURN m.title AS title'

`umx docs get time` has the full table; `umx docs get query` (Comparing dates) the Cypher side.

## Why

A made-up first day claims a precision the source never gave, and text comparison got mixed precisions wrong:
`2026-10-29T22:40:00Z <= 2026-10-29` was false and `2025 < 2025-02` was true. Comparing spans answers the same as
before for two stored dates of one precision, and correctly for mixed precisions and offset date-times, and CLI and
Cypher share one function so they never disagree.

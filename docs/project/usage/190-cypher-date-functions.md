---
id: 190
title: Ask date questions in umx query with the date functions, in universal time
route: query
status: current
source: D71 step 10, plan 201 (design note agreed by cypher-expert, pm-001, the architect and graph-storage)
reqs: []
supersedes: none
---
## Do
Use the date functions in `umx query` when a question needs now, a part of a date, a bucket or a gap:
`date(x)`, `timestamp(x)`, `current_date()`, `current_timestamp()`, `date_part(unit, x)`, `date_trunc(unit, x)`
and `date_diff(unit, a, b)`. Units are `year`, `quarter`, `month`, `week` (ISO, Monday first), `day`, `hour`,
`minute`, `second`, and `dayofweek` for `date_part` only (Sunday is 0).

Read every answer as universal time. `current_date()` is today's universal-time day, one instant for the whole
query, and `date("2026-10-30T09:40:00+11:00")` is `2026-10-29`. For a local day, compare with date-times that
carry their offset (`umx docs get time`).

Group by a bucket with `date_trunc` and count, and measure a gap with `date_diff`, which counts boundaries, except
week: whole 7-day spans, as DuckDB's `date_diff` does. `date_diff("month", "2025-01-31", "2025-02-01")` is 1;
`date_diff("week", "2026-12-13", "2026-12-14")` is 0 (Sunday to Monday) and 14 days is 2. `date_trunc("week", x)`
is still the Monday that starts x's week.

## Don't
Don't expect a part or a cut finer than the value: `date_part("day", "2025-01")` and `date_trunc("week", "2025")`
are null, because a month has no one day. A gap in days between two months is null too; ask in months.

Don't write `now()`, `datetime()` or `datediff()`: they are refused with a hint naming `current_timestamp()`,
`timestamp(x)` and `date_diff(unit, a, b)`. `duration.between(...)` does not parse. There is no `-` between dates.

Don't treat a null as "no date": text that is not a date (`"soon"`) gives null too, and so does a year outside 0001
to 9999. Dates before 1970 work like later ones. To find such values, run `umx check`: it names each node or edge
whose date field holds a value that is not a date (`date_not_a_date`, id and field), which compares as text, not
by date. Set a date there with `umx node edit <id> --set <field>=<date>` (`umx edge edit` for an edge).

## Example
```bash
umx query 'MATCH (a:action) WHERE date_diff("day", current_date(), a.due_on) >= 0 AND date_diff("day", current_date(), a.due_on) <= 7 RETURN a.title AS title, a.due_on AS due ORDER BY due'
umx query 'MATCH (m:meeting) RETURN date_trunc("month", m.starts_on) AS month, count(*) AS meetings ORDER BY month'
umx query 'MATCH (s:segment) RETURN s.title AS segment, date_diff("minute", s.starts_on, s.ends_on) AS minutes'
```

## Why
Dates already compared by instant (usage 161), but agents could not ask "due this week", "by month" or "how
long". The names are LadybugDB's, so a query that uses them reads the same in both engines; the results stay
text and numbers, so comparisons, sorting and grouping keep their existing rules.

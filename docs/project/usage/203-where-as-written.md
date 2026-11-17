---
id: 203
title: A --where value compares as written, and created and updated compare as dates
route: query
status: current
source: D82 (what changed recently), PR D82-1
reqs: [req-158]
supersedes: none
---
## Do
Write a `--where` value with or without quotes around it: one matching pair, `'...'` or `"..."`, is dropped, so
`--where "updated>='2026-11-12'"` finds the same nodes as `--where 'updated>=2026-11-12'`.

Filter on when a node was made or last changed with `created` and `updated`. They compare as dates, as spans of
UTC time, exactly as a declared date field does (usage 161): `updated>=2026-11-12` is from the start of that UTC
day, `updated=2026-11-12` is anywhere in it, `updated>2026-11-12` is after it, and a full date-time
(`updated>=2026-11-12T09:00:00Z`, or with an offset) works too.

When `where_invalid` says a field wants a date, give it one (`2026-01-15` or `2026-01-15T09:00:00Z`).

## Don't
Don't give `< <= > >=` a value that is not a date on a date field (declared, `created` or `updated`):
`updated>=next-week` or `due_on<soon` is refused with `where_invalid`, because comparing it as text would give a
silently wrong range. `=` and `!=` still compare such a value as text, so a stored non-date (a year `0000`, say)
stays findable with `=`. Don't nest quotes or mix them (`'2026-11-12"`): only one matching pair is dropped.

## Example
```bash
umx node list --where "updated>='2026-11-12'"          # changed on or after 12 November (UTC)
umx node list --where 'updated=2026-11-12'             # changed on that UTC day
umx node list --where 'created>=2026-11-01' --where 'created<2026-12'
umx node list --where 'updated>=next-week'             # refused: where_invalid, updated wants a date
```

## Why
`--where "updated >= '2026-11-12'"` returned every node with no warning: the quotes were kept as part of the value,
`created` and `updated` had no field kind, so the value was compared as text, and a quote sorts before every
digit. Taking the value as written and comparing the built-in timestamps as dates makes the quoted and unquoted
forms agree, and refusing a non-date in an ordering compare means a range never goes silently wrong.

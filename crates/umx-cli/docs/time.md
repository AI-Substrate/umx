# Time: dates, spans, and when things happened

umx has dates, not a time model. A field of kind `date` holds a year, a month, a day or an instant, and two
dates compare by when they are, not as text. When something happened is mostly in how you build the graph: date
a few anchor nodes and link the rest to them.

## The date kind

Declare a date field before you use it, on a type or on a relation's edges:

    umx ontology add field meeting starts_on:date
    umx ontology add field --relation works_for starts_on:date

In a std graph std has `starts_on` and `ends_on` on every node and edge already, so both lines are
`"unchanged": true` there and write nothing; declare any other date field the same way.

A value in a date field:

- A year (`2025`), a month (`2025-01`) or a day (`2026-10-30`) is stored as written. Write what the source
  says: a month or a year when that is all it gives, never an invented first day.
- A date-time is stored in universal time, with a `Z`, to the whole second. `2026-10-30T09:40:00+11:00` is
  stored as `2026-10-29T22:40:00Z`. The instant is kept; the offset you wrote is not. A view can show local time.
- A date-time with no offset is taken as universal time and warns `time_no_offset`. `2026-10-30T09:40:00` typed
  in Sydney is stored eleven hours out. Always write the offset (`+11:00`) or `Z`. The offset needs its colon:
  `+11:00` is read, `+1100` is not a date at all.
- Dates run from year 0001 to 9999, before 1970 too: `1066-10-14` is a day, and `1969-07-20T16:17:40-04:00` is
  stored as `1969-07-20T20:17:40Z`.
- Anything else (`2025-13`, `2025-1`, `2026-02-30`, year `0000`, or a date-time that falls outside years 0001
  to 9999 in universal time) is stored as given with `field_kind_mismatch`. It is not checked as a date.
- A value like that already in a date field (a year `0000` stored before the range was set, say) loads as written,
  never refused. It compares as text: `0000...` sorts before every date, so `--where 'seen_on<1970'` still finds
  it, and `=` matches it exactly. `umx check` names each one (`date_not_a_date`, node or edge and field); set a
  date there to fix it.

A field nobody declared is never checked or converted: `--set held_on=2026-10-30T09:40:00+11:00` keeps that
text, offset and all, with `off_vocabulary_field`. Declare the field first.

## Comparing dates

Each date is a span of universal time: a year, a month or a day is that whole period, a date-time its one
second. `--where` and `umx query` compare two dates by those spans, with one rule:

| Comparison | Holds when | Example that holds |
| --- | --- | --- |
| `a < b` | a ends by the time b starts (wholly before) | `2026-10-29T22:40:00Z < 2026-10-30` |
| `a > b` | a starts once b has ended (wholly after) | `2025-03 > 2025-02-28` |
| `a = b` | they overlap: one holds the other | `due_on=2025-01` finds `2025-01-15` |
| `a != b` | they do not overlap | `2025-01 != 2025-02-03` |
| `a <= b` | `a < b` or `a = b` | `2026-10-29T22:40:00Z <= 2026-10-29` |
| `a >= b` | `a > b` or `a = b` | `2025 >= 2025-06` |

The built-in `created` and `updated` compare the same way in `--where` (`updated>=2026-11-12`, quoted or not),
and an ordering compare with a value that is not a date (`updated>=next-week`) is refused with `where_invalid`.

So `2025 < 2025-02` does not hold (the year is not wholly before February), and `2025 = 2025-02` does. Two stored
dates of the same precision compare as they always did; pairs of different precisions changed (ws008 TM2), and so
did text written another way: a date-time with an offset compares as its instant, in `--where` and `umx query` alike.

Sorting (`ORDER BY`, `min`, `max`) goes by start, then the longer span first: `2025`, `2025-01`, `2025-01-01`,
`2025-01-01T00:00:00Z`, `2025-01-02`, `2025-02`. Any text that reads as a date compares as a date, also in a
string field or an undeclared one. A number is never a date: in `umx query` write a year as text (`"2025"`, or
`--param-str y=2025`).

## A plain date is a universal-time day

Because date-times are stored in universal time, a plain date means a day in universal time. The Friday 09:40
Sydney meeting, stored as `2026-10-29T22:40:00Z`, comes before `2026-10-30`. That is correct, and it is a trap:

    umx node list --type meeting --where 'starts_on<2026-10-30'    # includes that Friday morning meeting

To ask about a local day, compare with date-times. A date-time with an offset compares as its instant, and
`--where` can be repeated:

    umx node list --type meeting --where 'starts_on>=2026-10-30T00:00:00+11:00' --where 'starts_on<2026-10-31T00:00:00+11:00'
    umx query 'MATCH (m:meeting) WHERE m.starts_on >= "2026-10-30T00:00:00+11:00" AND m.starts_on < "2026-10-31T00:00:00+11:00" RETURN m.title AS meeting, m.starts_on AS starts ORDER BY starts'

A plain date sorts before every date-time on that day.

## Date functions in queries

`umx query` has LadybugDB's date functions. Every answer is in universal time; a date result is text, a part or a
gap a whole number.

| Function | Gives | Example |
| --- | --- | --- |
| `date(x)` | the value as a date at its own precision; a date-time's universal-time day | `date("2026-10-30T09:40:00+11:00")` is `2026-10-29` |
| `timestamp(x)` | the instant as `YYYY-MM-DDTHH:MM:SSZ`; a day's midnight | `timestamp("2026-12-01")` is `2026-12-01T00:00:00Z` |
| `current_date()`, `current_timestamp()` | now: one instant per query, the same on every row | `current_date()` is today's universal-time day |
| `date_part(unit, x)` (or `datepart`) | the unit's number | `date_part("week", "2027-01-01")` is 53 |
| `date_trunc(unit, x)` (or `datetrunc`) | the start of the unit that holds x | `date_trunc("week", "2026-12-17")` is `2026-12-14` |
| `date_diff(unit, a, b)` | how many unit boundaries lie from a to b, negative when b is earlier | `date_diff("month", "2025-01-31", "2025-02-01")` is 1 |

Units: `year`, `quarter`, `month`, `week` (ISO: weeks start on Monday), `day`, `hour`, `minute`, `second`, and for
`date_part` only `dayofweek` (Sunday is 0). A cut to `day` or coarser is a day (`2026-12-01`); to `hour` or finer, a
date-time.

- **Universal time.** `current_date()` can be yesterday or tomorrow where you are, and the Sydney Friday meeting
  above has `date(m.starts_on)` = `2026-10-29`.
- **A value is its whole span.** A part or a cut finer than the value is null: `date_part("day", "2025-01")` is null,
  because a month has no one day, and so is `date_part("hour", "2026-12-01")`.
- **A gap needs both values at the unit or finer.** `date_diff("day", "2025-01", "2025-03")` is null;
  `date_diff("month", "2025-01", "2025-03")` is 2.
- **Boundaries, except week: whole 7-day spans.** `date_diff("day", "2025-01-01T23:59:59Z", "2025-01-02T00:00:00Z")`
  is 1, and `date_diff("month", "2025-01-31", "2025-02-01")` is 1: the boundaries crossed, as DuckDB's `date_diff`.
  `week` counts whole 7-day spans between the two days, as DuckDB does: `date_diff("week", "2026-12-13",
  "2026-12-14")` is 0 (Sunday to Monday), 13 days is 1, 14 days is 2. Complete units (DuckDB's `date_sub`) are not
  provided.
- **Null and text that is not a date give null:** `date("soon")`. A bad unit or a number where a date goes is an
  error.
- **Before 1970.** Dates and date-times work from year 0001: `date_part("year", "1066-10-14")` is 1066, and
  `timestamp("1066-10-14")` is `1066-10-14T00:00:00Z`.

Actions due in the next seven days, and meetings counted by month:

    umx query 'MATCH (a:action) WHERE date_diff("day", current_date(), a.due_on) >= 0 AND date_diff("day", current_date(), a.due_on) <= 7 RETURN a.title AS title, a.due_on AS due ORDER BY due'
    umx query 'MATCH (m:meeting) RETURN date_trunc("month", m.starts_on) AS month, count(*) AS meetings ORDER BY month'

## Spans: starts_on and ends_on

`starts_on` and `ends_on`, on any node or edge, are its span in the world: when a meeting ran, a job was held, a
rule applies. They are the one pair; don't use `valid_from` or `valid_to` (a graph that already has them keeps
them until they are mapped across). Write them only when the source states them. `due_on` is a deadline.

When something was decided or said is never a date field. Link the point to where it was said (`raised_in`); a
status move records when the graph changed (`umx docs get node`), and `node status --in <anchor>` records where it
happened (the meeting, event or segment), so its said time comes from that anchor's dates. `created` and `updated` are when the graph
was written, in universal time, never when anything happened, and a node's id is no clock either.

## Planned times

For when something was meant to happen, use custom fields `planned_starts_on` and `planned_ends_on`, declared as
kind `date`. They are advice, not part of a standard vocabulary. `starts_on` and `ends_on` stay what actually
happened. What did not happen goes in the note, never in a time field.

    umx ontology add field segment planned_starts_on:date

## Replaced facts

When a fact is replaced, add a new node, link it to the old one with
`supersedes` (new to old), and move the old node to `superseded` with a reason. Declare `supersedes` and
`superseded` first, or the writes warn `off_vocabulary_relation` and `off_vocabulary_status`:

    umx edge add <new id> supersedes <old id>
    umx node status <old id> superseded --reason "Replaced by the January date (seq 410-415)"

## Month or year only

A date field takes the precision the source gives. For "January 2025", write the month; for "2025", the year.
Never invent a first day, and never add a `date_precision` field: the value carries its precision.

    umx node add --type event --title "Office move" --set starts_on=2025-01
    umx node list --type event --where 'starts_on=2025'      # anything in 2025, the month included

## Date the anchors, link the rest

Store a time once, on a few anchor nodes, and let everything else take its place through links. A date copied
onto every node goes stale and disagrees with itself.

1. Make the meeting or event a node, with `starts_on` and `ends_on` as full date-times with the offset.
2. For a long meeting, make each part a `segment` node, `part_of` the meeting, with its own `starts_on` and
   `ends_on` (full date-times) and a `step` (int) counting from 1 inside the meeting. Declare `segment` and
   `step`: they are std v1 names (`umx docs get organise`), not in the starter vocabulary. Use `precedes` only
   where there is no clock, or where two tracks run side by side.
3. Link every point to the finest anchor it came up in with `raised_in`: the segment, else the meeting.
   `umx node add --raised-in <anchor>` and `umx batch --raised-in <anchor>` add that edge in the same write as the
   node (`umx docs get batch`).
4. Give a transcript node a `starts_on`, and keep citing passages by sequence range (`umx docs get evidence`).
5. Steps of a process or a day held as values in a field: make them nodes (`segment`, `phase`, `method`), each
   with its `step`, and link to them.
6. Say the graph's order in its conventions, in one line:

       umx graph set demo --conventions "Spine: segment by starts_on, inside meeting; points placed by raised_in."

When a status moves because of something said, put the position in the reason: `--reason "... (seq 410-415)"`.
Never guess a date, and never write a time as text ("09:40" in a string field).

## Warnings when you write

umx checks time as you write, on `node add`, `node edit`, a batch's node lines and an edge's fields. Each is a
warning: the write goes ahead. In a batch it carries the line it came from.

| Warning | When | Fix |
| --- | --- | --- |
| `time_no_offset` | a date field is given a date-time with a clock and no offset (`2026-10-30T09:40`) | Write the offset: `2026-10-30T09:40:00+11:00`. Bare, it is stored as universal time, hours out for a local time |
| `time_as_text` | a text field holds a time: its value reads as a date or a clock time (`2026-10-30`, `9:40 am`), or its name ends in `_at` or `_on`. Text is kept as written: never normalised to universal time, and a clock time alone never compares as a time | Use `starts_on` and `ends_on`, or declare the field kind `date` (its next does, for an undeclared field) |
| `anchor_undated` | a `meeting`, `event`, `segment` or `transcript` has no `starts_on`: when it is added, or when an edit makes a node one or removes its date | Date it from the source, a full date-time with its offset (its next is the `node edit`) |

A plain date (`2026-10-30`), a month or a year has no clock, so it never warns `time_no_offset`. A bare year in a
text field is left alone: four digits are often not a date.

## Spines: propose, then confirm

A spine is an order the graph can be laid out along: its anchors (the nodes that are the bands), what orders them
(a date field, a `step`, or a chain such as `precedes`) and which links place other nodes on them. Ask umx which
spines the graph could have:

    umx graph spine propose demo

It lists each candidate with its cover (how many nodes it places by their own field, by a link, by evidence
position, by rule, and how many it leaves out) and its spread, offers only those that spread the nodes over more than one
band, and says what is in the way: steps held as field values, step numbers that disagree with `precedes`, and
points linked to one segment whose evidence sits in another. `umx graph guide` and `umx check` show the best one.

- A `step` orders only among the anchors with the same `part_of` parent, so step 1 of each day is fine, and two
  segments with the same step under one parent are side by side, not an error.
- Only links inside the graph place a node; a link to another graph's segment does not, yet.
- Two evidence pointers are compared only in the same source and unit (`seq` with `seq`, lines with lines), with
  the source in its normal form (`./t.ndjson` and `t.ndjson` are one). A `umx:` source is never compared.

- A node nothing else places, but that links to nodes placed by field, link or evidence, is placed by rule: in the
  middle of their bands (each neighbour once), counted as `by_rule`. A rule placement never places another node,
  never counts toward a spine's spread, and the anchors' parents (the meetings) are never placed by rule. List
  them with `umx graph spine propose demo --by-rule <candidate>` or `umx graph spine show demo --by-rule <name>`.
- On a spine ordered by a date, a node that holds that date itself sits in the band of the last anchor dated at or
  before it, and counts toward the spread; one dated before every anchor sits before them all. While no anchor
  holds that date (anchors ordered by step only), a node's own date places nothing: link it to its anchor.

Proposing stores nothing. Declare the spine you choose, so the graph says how it is ordered:

    umx graph spine confirm demo type:segment
    umx graph spine set demo --file day-spine.json
    umx graph spine show demo

`confirm` stores a proposed candidate's declaration unchanged; `set` stores one you write (to make it the `main`
spine, or to change its order or place). Every name in it must be the graph's vocabulary or std. `show` places the
graph on each declared spine as it is now, with cover and spread. A graph has at most one main spine. Declaring
the first spine moves graph.json to format 6, which an older umx will not load (`umx docs get graph`).

The names `recorded`, `status` and `date_field:<f>` are the built-in spines below, so a declaration may not take one
(`spine_name_reserved`). A graph that declared one before umx reserved them still loads; `umx check` warns
`spine_name_reserved_stored`, and `--along` with that name reads the declared spine.

## Reading along a spine

Every read that lists nodes or edges takes a range on one spine, in place of an "as at" question: what came up
before lunch, between two segments, after a date.

    umx node list --along day --after <segment> --before <segment>
    umx node list --along day --before <segment> --type decision
    umx edge list --along day --after <segment>
    umx node find "budget" --along day --after <segment>
    umx search "vendor delay" --along day --before <segment>
    umx query 'MATCH (d:decision)-[:raised_in]->(s) RETURN d.title, s.title' --along day --after <segment>
    umx node list --along date_field:due_on --after 2026-12-01 --before 2026-12-31
    umx node list --along recorded --after 2026-11-12T09:00:00+11:00

- `--along <spine>` names a declared spine (`umx graph spine show`) or a built-in: `recorded` (when each node was
  written), `status` (its latest status move) or `date_field:<f>` (a date field the graph declares and some node
  holds). `source` is not a range.
- On a declared spine an end is one of its anchors: an id, a unique prefix, or `umx:<this graph>/<id>`. On a
  built-in an end is a date in any form umx reads (`2026-11`, `2026-11-12`, a date-time with its offset).
- Both ends are inclusive. `--after s2 --before s3` keeps what sits in segments 2 and 3; `--before 2026-11-12`
  keeps the whole of that universal-time day. Give either end alone, or neither (everything the spine places).
  On a built-in a value overlaps an end: a `due_on` of `2026-11` is on or before `2026-11-12` and on or after it.
- Only nodes the spine places by data are kept. Nodes it places only by rule, and nodes it does not place, are left
  out and counted: the reply's hints `along_by_rule` and `along_not_placed` give the count and the command that
  lists them (`umx graph spine show <graph> --by-rule <spine>` or `--unplaced <spine>`). For node list, node find
  and edge list they count the rows the other filters kept; for search and query, the graph's nodes.
- The range is applied before paging: `total` and `--offset` count the rows in range.
- An edge is kept when both its ends are kept. A query sees only the nodes in range and the edges between them: no
  path passes through a node outside the range, and an `OPTIONAL MATCH` to one gives null.
- Search narrows each of its legs to the range before it ranks, so strong matches outside the range never crowd
  out weaker ones inside it.
- Reply data carries `along`: the spine, whether it is `declared` or `builtin`, its clock, the ends as umx read them,
  `ends: inclusive`, the `not_placed` and `by_rule` counts, `coarse` (values compared at month or year precision)
  and `spines` (how many other spines the graph has).
- One graph only: `--along` with `--graph a,b`, `--scope`, `--all` or `--provenance` is refused. An end that is not
  one of the spine's anchors, or not a date on a built-in, is `along_end_invalid` with the first anchors as
  candidates; ends in the wrong order are refused too.

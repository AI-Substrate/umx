---
id: 178
title: Fix the time warnings a write gives
route: build
status: current
source: ws008 TM3, plan 173 (the project manager 2026-11-13; D71 R4; the time review's step 6)
reqs: [req-147]
supersedes: none
---
## Do
Read the time warnings on `node add`, `node edit`, `umx batch` and `edge add`, and fix them in the next write. `time_no_offset`: you gave a date field a date-time with no offset, so it was stored as universal time; write it again with the offset the source gives (`2026-10-30T09:40:00+11:00`; the colon is needed, `+1100` is not a date). `time_as_text`: a text field holds a time (its value reads as a date or a clock time, or its name ends in `_at` or `_on`); move it to `starts_on` and `ends_on`, or run the `umx ontology add field <type> <field>:date` its next gives. `anchor_undated`: a meeting, event, segment or transcript has no `starts_on`; date it from the source with the `node edit` its next gives.

## Don't
Don't guess a date to silence `anchor_undated`: no date is better than a wrong one, so leave it undated when the source says nothing. Don't write a local time without its offset. Don't keep a time in a text field.

## Example
```bash
umx node add --graph launch --type meeting --title "Kickoff" --set starts_on=2026-10-30T09:40   # warns time_no_offset
umx node edit umx:launch/01JC8XA4H7 --set starts_on=2026-10-30T09:40:00+11:00
umx ontology add field risk review_on:date --graph launch   # the time_as_text next
```

## Why
umx stores time as universal time (D71 R4), so a bare local time lands hours out; a time kept as text is stored as written, never normalised to universal time as a date field's is (a clock time alone never compares as one); and an anchor with no date cannot be placed on a time spine. The warnings catch each where it is written, when the fix is one command.

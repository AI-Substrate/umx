---
id: 010
title: Statuses move, with a reason
route: build
status: current
source: req-12 (s05b); live-concept-graph method
reqs: [req-12]
supersedes: none
---
## Do
When an argument moves, move the status with `umx node status <id> <status> --reason "..."`: open or contested to agreed is RESOLVED; agreed back to contested is IN CONTENTION; ruled_out back to open or proposed is BACK ON THE TABLE. A contradicted fact goes to contested with both values in the note.

## Don't
Don't leave something agreed because it was agreed once. Don't use contested for 'someone disagreed'; it means the argument is live and unresolved. Don't overwrite a contradicted value silently.

## Example
```bash
umx node status <id> contested --reason "Finance disputes the date"
```

## Why
Status history is the most valuable record of how a room changed its mind.

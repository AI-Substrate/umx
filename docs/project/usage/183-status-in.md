---
id: 183
title: Say where a status move happened with node status --in
route: build
status: current
source: ws008 TM6, plan 183 (the project manager 2026-11-13; D71: said time comes from raised_in and the status move)
reqs: [req-150]
supersedes: none
---
## Do
When a decision is made or a status changes in a meeting, record the move with where it happened: `umx node status <node> <status> --reason "<why>" --in <anchor>`, the anchor being the meeting, event or segment (a node of the graph, or `umx:<graph>/<id>` in another). Add `--evidence <pointer>` (and `--evidence-kind`) to cite the passage; the pointer records the session that added it (`by_session`), as node evidence does. In a batch, give a `node status` line `"in"` and `"evidence"`. `node get` shows each move's `in` and `evidence` in `status_history`.

## Don't
Don't put when something was decided in a date field: the move's `in` and the anchor's dates say it. Don't expect a batch's `--raised-in` to set a status move's `in`: name it on the line. Don't cite a pointer that holds a secret: it warns `secret_suspected`, and a batch refuses it. Don't give `--evidence-kind` a kind umx does not know, even without `--evidence`: it refuses and nothing moves. Don't expect a move's evidence in a node's `evidence_kinds`, in `umx node evidence <node> list` or in provenance reads: it lives on the move (`status_history`), not in the node's evidence. A `node evidence <node> remove --redact` of its source does scrub it, and keeps the move (usage 175).

## Example
```bash
umx node status umx:launch/01JC8XA4H7 agreed --reason "Signed off in the review" --in umx:launch/01JC8XB2K9
umx node status umx:launch/01JC8XA4H7 stalled --reason "Waiting on the vendor" --in umx:launch/01JC8XB2K9 --evidence "calls/review.vtt#seq=40-52" --evidence-kind room
```

## Why
The act time of a decision is "said" time (D71): it comes from where the move was made, never a field. Naming the anchor on the move lets a time spine place the decision, and the anchor resolves as an edge add end does, so a wrong one fails before anything is written.

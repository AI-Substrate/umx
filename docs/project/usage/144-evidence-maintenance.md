---
id: 144
title: Remove stale or wrong evidence with umx node evidence remove --reason
route: build
status: current
source: ws006 S1b (plan 116); graph-addressing-expert topic:evidence-maintenance (2026-11-11); F100 (field report)
reqs: [req-126]
supersedes: none
---
## Do
See a node's pointers with `umx node evidence <node> list`: each row has its position (`at`) and its `pointer` in
canonical form. Remove one with `umx node evidence <node> remove '<pointer>' --reason "<why>"`, copying the pointer
from list (single-quote it in the shell). Matching compares the source and the selectors in canonical form, so
`quote=we%20ship` and `quote=we ship` are the same pointer, and the kind is ignored. If two live pointers match,
umx refuses with `evidence_ambiguous` and lists their positions; add `--at <n>` with one of them. Add one pointer
with `umx node evidence <node> add '<pointer>'` (the same checks as `node note --evidence`). For many removals, use a
batch with one reason: `umx batch --file fixes.jsonl --reason "question lines are not evidence"`, one line each,
`{"op":"evidence remove","node":"<id>","pointer":"<pointer>"}`, plus `"missing":"skip"` on a line that may already
be gone. Check what was removed with `umx node evidence <node> list --removed`.

## Don't
Don't hand-edit nodes.jsonl to drop evidence: nothing records what went or why, and a slip can break the line.
Don't remove without a real reason; `--reason` is required (1 to 500 characters, no line breaks, control, bidi or
zero-width characters) and is kept on the node. Don't add or strip `?repo=` to make a `umx:` pointer match: the
hint is not part of the match, and a `umx:` id prefix matches the full id stored on that node (as text, never
looked up). Don't rely on positions across writes; they shift, so `--at` only picks among pointers that match.

## Example
```bash
umx node evidence 01JC8XA4H7 list
umx node evidence 01JC8XA4H7 remove 'calls/3-nov.ndjson#seq=120-140' --reason "a question, not an answer"
umx node evidence 01JC8XA4H7 remove 'calls/3-nov.ndjson#seq=120-140' --reason "a question, not an answer"   # already_removed: nothing changes
umx node evidence 01JC8XA4H7 list --removed
```

## Why
Nothing is erased. A removed pointer moves to the node's `evidence_removed` trail as
`{removed, removed_at, reason, by, by_session}`, so a later reader sees what was taken out, when and why. `removed`
is the pointer exactly as stored, never flattened. Two paths name two sessions:
`evidence_removed[i].by_session` is the session that removed it, and `evidence_removed[i].removed.by_session` the
session that added it (absent for a pointer stored before sessions were recorded). Repeating a removal reports
`already_removed`, so a clean-up script can run twice. Matching is on text only and never looks up a `umx:`
pointer's target, so it says nothing about another graph, secured or not.

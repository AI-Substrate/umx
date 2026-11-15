---
id: 181
title: Read hints as advice; they never mean something went wrong
route: output
status: current
source: ws002 X1, plan 180 (architect topic:hints-envelope, D69; the project manager 2026-11-13 on which codes move)
reqs: [req-139]
supersedes: none
---
## Do
Read the reply's `hints` list. Each hint has `code`, `message` and `next`, like a warning, and names a good next step: link a segment to its meeting (`segment_without_parent`), describe a graph (`purpose_missing`, `descriptions_missing`), reuse a node that already exists (`near_duplicate`), give a status move its reason (`reason_missing`). Run its `next` when it fits the job.

A hint never changes `status` or the exit code: an `ok` reply with hints is still `ok`. `hints` is left out when there are none. A reply lists at most 8, in catalogue order, then `hints_truncated` with how many more there were. A batch lists its hints after its warnings, each with its `line`, and they never count toward `batch_warnings_truncated`.

These codes are hints (they were warnings before protocol 71): `purpose_missing`, `descriptions_missing`, `reason_missing`, `near_duplicate`, `near_duplicate_line`, `find_no_match`, `resource_elsewhere`, `entity_key_is_title`, `graph_loaded_on_demand`, `graphfile_no_repo`. New: `segment_without_parent` and `hints_truncated`. `umx docs get errors` lists every code with its kind.

## Don't
Don't treat a hint as a failure, and don't retry a command because it gave one. Don't silence a hint by inventing what it asks for (a made-up purpose or description). Don't read `warnings` alone: advice moved to `hints`.

## Example
```bash
umx node add --type segment --title "Opening remarks"
# status ok; hints[0] segment_without_parent, next: umx edge add <id> part_of <parent> --graph demo
umx edge add 01JC8XA4H7 part_of 01JC8XB000 --graph demo
```

## Why
A warning should mean that something you asked for did not fully happen. Advice reported as a warning turned every reply into `status: warning`, and agents learned to skip warnings (architect ap-37). Hints keep the advice, with a command to run, and leave `status` meaning what it says.

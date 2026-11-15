---
id: 113
title: Fix a wrong edge with edge edit or edge delete, never by editing edges.jsonl
route: build
status: current
source: F75 (field reports from two testers, project manager 2026-11-11), plan 086
reqs: [req-13, req-59, req-65, req-88, req-89]
supersedes: none
---
## Do

When an edge has the wrong relation or points the wrong way, fix it in place. Find its id with
`umx edge list --from <node>` (edge ids are full ids or addresses, never prefixes), then:

- `umx edge edit <edge> --relation <r>` to retype it,
- `umx edge edit <edge> --reverse` to swap its ends,
- both flags together to do both,
- `umx edge delete <edge>` to remove it.

The edge keeps its id, so anything that names it still works. Read the `sentence` in the reply to check the
direction, as after `edge add`. For many fixes at once, put `edge edit` and `edge delete` lines in a batch file
(`{"op":"edge edit","edge":"<id>","relation":"<r>","reverse":true}`, `{"op":"edge delete","edge":"<id>"}`): all
or nothing, like every batch.

What is recorded: an edit that changes the edge stamps it with `updated` (the time) and `updated_by` (`--by`,
else `UMX_AUTHOR`, else the OS user), and `changed` in the reply says `type`, `direction` or both. That is all.
There is no history of earlier relations or directions, and a delete leaves nothing in the graph; the reply
carries the deleted edge, so keep it if you need it.

## Don't

Don't edit edges.jsonl by hand to fix an edge, and don't delete and re-add it (that changes its id).
Don't use `updated` or `updated_by` as custom edge fields: they are reserved now (`reserved_name`). An older
graph that has its own field by either name is listed by `umx check` as `edge_reserved_field_in_use`, and
`edge edit` refuses that edge until the field is renamed.
Don't retry these refusals unchanged; nothing changed:
- `edge_would_duplicate`: the fixed edge already exists (it names it). Delete this one instead.
- `edge_derived`: umx keeps `similar_to` edges (`umx similarity rebuild`) and the provenance relations
  `authored_by` and `edited_by` itself.
- `edge_repeated`: the edge's id is on more than one line of edges.jsonl (after a git merge); deleting the
  line in use would bring an earlier one back. Run `umx check`, keep one line for the id, then delete.

Don't retype an edge into `authored_by` or `edited_by` (or `similar_to`): umx derives those itself and refuses
them (`name_invalid`), on `edge add` too.

`--reverse` on a symmetric relation (`conflicts_with` and the like) has nothing to reverse: it warns
`edge_reverse_symmetric` and changes only the relation, if you gave one. A retype to one stores the lower id as
`from`; when that swaps the ends, the hint `edge_direction_normalised` names them as stored.

## Example

    umx edge list --from 01JC8XA4H7
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --reverse
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --relation supports --by alice
    umx edge delete 01JC8XE5RT2W9V8B3N4M5K6J7H

## Why

Agents using umx fixed wrong edges by hand-editing edges.jsonl, which bypasses every check (duplicates,
symmetric relations, derived edges) and leaves no record of who changed what. F75 gives the fix a verb with the
same rules as `edge add`.

---
id: 189
title: Change an edge's custom fields in place with edge edit --set and --unset
route: build
status: current
source: F134, F161, plan 199
reqs: [req-13]
supersedes: none
---
## Do

Change a field on an edge you already have, keeping its id:

- `umx edge edit <edge> --set <field>=<value>` sets one (repeat `--set` for more). On a field the relation does not
  declare, a raw `--set` value's type is guessed (a number, `true`/`false`, else text; `null` stays text).
  `--set-str` keeps text as text and `--set-json` takes JSON: use them to keep a value's exact type.
- `umx edge edit <edge> --unset <field>` removes one.
- Mix them with `--relation` and `--reverse` in one edit. A batch `edge edit` line takes `sets` and `unsets`.

End a membership by setting `valid_to` on its edge, rather than deleting the edge. Correct an edge's `why` the same
way. The reply's `changed` names each field that took a new value or was removed. The edge is stamped `updated`,
`updated_by` and `edited_by_session`. An edge with an end in another graph is edited from the graph that stores it.

## Don't

Don't delete and re-add an edge to change a field: the new edge gets a new id and creation stamp, and anything that
named the old id loses it. Don't set reserved names (`type`, `from`, `to`, `created`, `updated`, `evidence` and the
like): they are refused with `reserved_name`. The one exception is `--unset evidence` on an older edge whose own
`evidence` field is not an evidence list. Move that value first with `--set-json why=<value>`, as
`evidence_field_invalid` says.

## Example

    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --set valid_to=2026-11-30
    umx edge edit umx:crm/01JC8XE5RT2W9V8B3N4M5K6J7J --set-str why="Named in the kickoff notes" --unset confidence

## Why

`edge edit` could only retype or reverse an edge, so agents ended a membership or fixed a `why` by deleting and
re-adding the edge, which lost its id. The same checks as `edge add --set` apply: the relation's declared fields type
the value, undeclared ones warn `off_vocabulary_edge_field`, date fields are read as add reads them, and a value that
looks like a secret warns `secret_suspected`.

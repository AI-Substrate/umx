---
id: 137
title: Give a node type its own node list columns with umx ontology set type
route: build
status: current
source: F96 (a tester's C2, the project manager and the architect, 2026-11-11), plan 108
reqs: [req-08, req-28]
supersedes: none
---
## Do

When a type's useful state is in a custom field (a risk's `likelihood`, a task's `owner`), set its default list
columns once:

    umx ontology set type risk --columns title,likelihood,impact

From then on `umx node list --type risk` shows `id`, `title`, `likelihood` and `impact`. Columns may be any standard
node field (`title`, `status`, `updated`, `evidence_kinds`...) or a field the type declares; declare a new one
first with `umx ontology add field risk category:string`. `umx ontology show` lists each type's `columns`.
`umx ontology set type risk --reset` goes back to the usual `id, type, title, status, updated`.

## Don't

Don't expect the type's columns without `--type`: a list of mixed types keeps the usual columns. Don't combine
`--columns` and `--reset` (`usage_invalid`). An explicit `node list --columns` always wins over the type's list, so
pass it when a script needs fixed columns. Don't name an undeclared field: it is refused with `field_missing`. A type takes at most 32 columns, and never a provenance
field (`prov_key`, `authored_by_session`, `edited_by_session`): both are refused with `usage_invalid`. If
`graph load` warns `type_column_unknown`, the type lists a field it no longer declares (a hand edit); node list
skips it until you set the list again.

## Example

    umx ontology add field risk likelihood:string --values low,medium,high
    umx ontology set type risk --columns title,likelihood
    umx node list --type risk                    # id, title, likelihood
    umx node list --type risk --columns id,status   # the explicit list wins

## Why

Field use (F96): `node list --type <t>` showed status, while the state that mattered for that type sat in a
custom field visible only with `--columns` on every call. Types and their custom fields are declared per graph
(req-08), and the everyday list verb (req-28) should show what an agent needs without another call.

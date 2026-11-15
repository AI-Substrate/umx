---
id: 169
title: See every custom field on node list and node get with --columns all
route: explore
status: current
source: F137 (field reports, project manager, 2026-11-12), plan 151
reqs: [req-59]
supersedes: none
---
## Do

To see the fields a graph's nodes carry beyond the standard ones (`owner`, `likelihood`, any `--set` field), ask
for all of them:

- `umx node list --columns all`: the usual columns (`id, type, title, status, updated`, plus `graph` and
  `address` across graphs), then every custom field any node on the page has, sorted by name. A node without one
  of them has null there. A page whose nodes have no custom fields gives exactly the usual columns.
- `umx node get <node> --columns all`: the same for one node: the usual columns and that node's own custom fields.

`--columns all` overrides a type's own default columns (`umx ontology set type <t> --columns`, usage 137), so
`umx node list --type risk --columns all` shows every field of the risks on the page. It is the same rule as
`umx edge list --columns all` (usage 121).

## Don't

Don't mix `all` with other names: `--columns all,owner` is refused as usage (`usage_invalid`); give `all` alone,
or name the columns you want. Before this, `all` was read as a field named `all` and came back null on every row.
Don't expect a custom field named like a column umx computes (`copy`, or a reserved name in a hand-edited graph)
to appear: the computed column wins. A plain unknown name in `--columns a,b` is still null, not an error.

## Example

    umx node list --type risk --columns all
    umx node get 01JC8XA4H7 --columns all
    umx node list --columns id,title,owner

## Why

Several agents using umx asked for `--columns all` on node list and got status ok with one column named `all`, null
everywhere. Edge list already had it (F74); nodes now match.

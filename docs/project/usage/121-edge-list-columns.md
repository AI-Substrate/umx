---
id: 121
title: See an edge's custom fields with umx edge list --columns
route: explore
status: current
source: F74 (field report, project manager, 2026-11-11), plan 094
reqs: [req-13, req-59]
supersedes: none
---
## Do

When an edge carries fields of its own (set with `--set` on `edge add`, such as `confidence` or `source`), ask
for them on the list:

- `umx edge list --from <node> --columns all`: the usual columns, then every custom field found on the page, sorted
  by name. An edge without one of them has null there.
- `umx edge list --from <node> --columns to,to_title,confidence`: exactly those columns, as `node list --columns`
  works. Any row column, `created`, `created_by` or a custom field; a name the edge lacks is null.

It works with `--to`, `--type`, `--limit`, `--offset` and multi-graph scopes (which still add `graph` and
`address`). JSON rows carry the values; the human table shows them as columns.

## Don't

Don't read `edges.jsonl` to find a field: that was the only way before, and it skips the daemon's view (titles,
external ends, scopes). Don't expect custom fields in the default rows: without `--columns` the rows are unchanged.
A custom field named like a row column (`sentence`, `to_title`, `from_state`) is never shown: the row's column
wins, and stays null where umx leaves it out on purpose (a hidden or unloaded end has no title).

## Example

    umx edge add <from id> blocks <to id> --set confidence=0.8
    umx edge list --from <from id> --columns all
    umx edge list --from <from id> --columns to_title,confidence

## Why

Field use (F74): an agent set `confidence` on edges and could not read it back through umx; it was only in
edges.jsonl. Edges are first class (req-13) and read back as sentences (req-59), so their fields belong in the
list too, on request, matching node list's `--columns`.

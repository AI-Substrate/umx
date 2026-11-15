---
id: 140
title: A next that carries a list prints it with commas; run it as printed
route: output
status: current
source: F73 (the project manager 2026-11-12, approved by the architect and security), plan 110
reqs: [req-40, req-41]
supersedes: none
---
## Do

Run a `next` exactly as umx printed it. When it carries a list of names, the list keeps its commas:
`--columns id,title`, `ontology add field risk,issue <field>:<kind>`, `--graph alpha,beta`. Before F73 such a
list came out as `id?title`, or with spaces in place of commas, or the `--graph` list was dropped.

## Don't

Don't fix up a next by hand. A `?` in a next still means umx left out a value it could not check (free text such as a
title never goes into a next), so put your own value there. Don't expect commas anywhere else in a message:
only a whole list of plain snake_case names keeps them.

## Example

    umx node list --columns id,title
    # graph_ambiguous: next is umx node list --columns id,title --graph beta
    umx node list --columns id,title --graph beta

## Why

A next is meant to run as printed (req-40). umx re-checks every next before printing it (req-41), and that check
used to turn every comma into `?`. Now one rule admits commas: a whole token that is 2 to 64 snake_case names joined
by commas. Free text still never carries a comma, and a model key placeholder in a next reads `<model_key>`.

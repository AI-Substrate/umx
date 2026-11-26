---
id: 230
title: Copy graph-local rule sets and keep risks owned
route: build
status: current
source: PRD 003 rules-24, rules-28 and rules-29
reqs: [rules-24, rules-28, rules-29]
supersedes: 227
---
## Do

Read `umx docs get rules`. Copy a set with `umx rule set add <file> --graph <g>`;
its snake_case stem names `<graph>/rules/<set>.json`, never `std`. List the copies and
their row counts with `rule set list`; remove one with `rule set remove <set>`.
Copy the same source into each graph separately. The copy never follows later source changes.
Byte-identical re-adds are unchanged; remove a different old copy before adding its replacement.

Set files contain only `rules` and `rules_off`. A row is addressed as `<set>:<rule>`.
Rules compose as std, filename-ordered sets, then own rows. Every source's switch-offs
join one union and suppress any matching name; unknown names are kept.
An unreadable set contributes neither rows nor switch-offs and warns once with its reason.
Other sets and the graph still load.

Std graphs now ship `std:risk_owned`. A risk needs an `owns` edge from a person,
organisation or team, never an `owner` string. An unmet rule warns and the write stands.
Follow its edge-add next, or deliberately switch it off for one graph with
`umx rule off std:risk_owned`. Non-std graphs acquire no central rule.

Checks reserve actual per-node bounds after exact filters, with one unit for a matching
type-only rule. More than 100 cheap rules are supported. A pathological node keeps
completed rule results, marks remaining unaffordable checks `not_checked`, and advances.
Follow `next` and `resume`; incomplete totals remain null.

## Don't

Do not link a source file into `rules/`, put a hub or fill in a set, or edit another
graph's copy expecting this one to change. Do not treat a warning as a failed write.
Do not use a set where an older reader must still write the graph: CLI addition raises
graph.json to format 10.

Each set is bounded to 64 rows and 64 KiB. Only the first 32 valid-name regular files
are used; omitted files are counted in one warning, and invalid names do not consume
the limit. Above 1000 directory entries the whole folder is excluded, never a refused load.
An add past either folder limit is refused without writing.

## Example

```bash
umx rule set add ./risk_management.json --graph demo
umx rule list --graph demo
umx rule show risk_management:rated --graph demo
umx check --rules --rule risk_management:rated --graph demo
umx rule set remove risk_management --graph demo
```

`risk_management.json`:

```json
{"rules":[{"name":"rated","match":{"type":["risk"]},"field":"impact"}],"rules_off":["std:risk_owned"]}
```

## Why

Shape advice travels with one committed graph, without machine-wide configuration or
automatic mutation. Protocol 109 adds rule-set commands and bounded-copy diagnostics;
existing warning fields stay unchanged. This supersedes usage 227's empty-central-table
note: `std:risk_owned` now supplies the default owner reminder.

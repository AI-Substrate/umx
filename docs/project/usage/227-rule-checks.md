---
id: 227
title: Check graph rules softly without refusing writes
route: build
status: current
source: PRD 003 rules-07 through rules-14
reqs: [rules-07, rules-08, rules-09, rules-10, rules-11, rules-12, rules-13, rules-14]
supersedes: 225
---
## Do

Read `umx docs get rules` before declaring shape. Use `umx rule add`, preview with
`--dry-run` or `rule show`, and inspect declarations with `rule list`. Undeclared vocabulary
warns but is kept; declare it if it fits your graph. The production central table is empty.
Rules now warn, never refuse or create edges, when a node add/edit or an edge deletion/retype/reversal
leaves a matching node without its required field or edge. Edge changes check both local ends;
a batch checks the final state once. Other writes do not run rules.

`rule_unmet` keeps exit code 0 and the exact fixed sentence also shown by `rule list` and
`graph guide`. Only this warning gains `rule: {name, count, nodes}`; node ids are capped at
100 while count keeps the total. Single writes name the node as subject and give its fix.
Batches give one warning per unmet rule and a filtered check command. Descriptions remain data.

Plain check includes per-graph unmet-node summaries. `--rules` lists at most 100 unmet or
`not_checked` findings, each with a safe suggested command. `--offset` pages known findings;
`--after` resumes a cut scan in node-id order. Follow the returned `next`/`resume` commands.
The 100-million-unit budget is shared across rules; an unaffordable node is reported and
skipped without stopping later nodes. Incomplete totals are null. Checks reread disk and never
write. Dry-run batch warnings replace provisional ids with `line:N`.

Symmetric edges count in either direction. A type-based shape counts an external far end
without consulting another graph; exact targets stay local. Provenance is excluded.

## Don't

Do not treat a warning as a refused write, retry a successful write, or expect rules to add
edges. Do not treat a cut scan as complete. Do not put secrets in patterns or descriptions.
Do not add rules if an older umx must still read the graph: the first stored rule raises
graph.json to format 8. Named rule sets and automatic hubs are separate work.

## Example

```bash
umx rule add risk_rated --type risk --field impact --graph demo
umx check --rules --graph demo
umx check --rules --rule risk_rated --graph demo
```

## Why

Shape advice belongs in the committed graph, without fencing off legitimate work.
Protocol 107 adds Check's `rules`, `rule`, `after`, `data.rules`, `rule_unmet`, `rule_check_cut`,
and the optional warning context. Existing warnings gain no fields. Hubs and automatic joins
are separate work; rules remain warning-only.

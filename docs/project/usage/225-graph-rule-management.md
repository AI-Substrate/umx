---
id: 225
title: Declare graph rules and preview their patterns before changing graph shape
route: build
status: current
source: Graph rules design, 2026-11-26
reqs: []
supersedes: none
---
## Do
Read `umx docs get rules`. Add a field or edge rule with `umx rule add`; undeclared vocabulary
is stored with `rule_vocabulary_unknown` and a declaring next, not refused. Declare it if that
matches the graph's design. Try the pattern with `--dry-run`, inspect matching node ids
with `umx rule show`, and list the stored declarations with `umx rule list`. Use bounded Rust
regex over title, note or a string field, and exact values with `--where`. Re-running an
identical add writes nothing. Central rules, when shipped, can be switched off and on by
their `std:<name>`; the current central table is empty.
Use at most four regex fields per rule. A node over a work limit is skipped, never a reason
to stop checking later nodes. `rule_node_unchecked` and `rule_scan_cut` name the incomplete
preview: its total is unknown, `not_checked` lists up to 100 ids and `not_checked_total`
counts them all. Stored over-budget patterns load with
`rule_skipped`, remain on disk and can be removed using the warning's next command.

## Don't
Do not expect write-time warnings, automatic fill or named rule sets yet. Do not store a
rule when older umx must still read the graph: its first rule or switch-off raises the
format to 8. Do not treat a cut preview as complete or expect a next page for an unstored
dry-run. Do not put secrets in patterns or descriptions; they are graph text.

## Example
```bash
umx ontology add field risk rating:string --graph demo
umx rule add rated --type risk --field rating --graph demo --dry-run
umx rule add rated --type risk --field rating --graph demo
umx rule show rated --graph demo
umx rule remove rated --graph demo
```

## Why
The graph's desired shape belongs to its committed files, not to machine config. Previewing
the pattern makes the selection visible before the rule is stored.

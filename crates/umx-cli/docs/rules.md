# Rules: graph shape as data

A rule names the nodes it is about and one thing those nodes should have: a field, or an edge.
Rules warn on writes and can be checked explicitly. They never create or remove edges.
No write is refused because a node does not meet a rule.

## Add and inspect

    umx rule add risk_rated --type risk --field rating --graph demo
    umx rule list --graph demo
    umx rule show risk_rated --graph demo --limit 50 --offset 0
    umx rule remove risk_rated --graph demo

To declare `rating`, use `umx ontology add field risk rating:string --graph demo`.
Rule names are snake_case. A different row under an existing name is refused; remove it before
adding the replacement. Adding the identical row returns `unchanged: true` and writes nothing.
Removing an absent graph rule is also unchanged.

`--graph` chooses one graph. Without it, normal graph selection applies. `--by` records the caller;
`--scope` is not supported for rule management. The graph stores at most 64 rules of its own.

## Check and fix

    umx check --rules --graph demo
    umx check --rules --rule risk_rated --graph demo
    umx check --rules --rule risk_rated --graph demo --offset 100

Plain `check` includes each graph's known count of unmet nodes and the command that lists them.
`--rules` returns up to 100 finding rows, with rule, node, state, fixed sentence and a suggested
node edit or edge add. Commands contain the real node id and placeholders for values still
needed. They never execute automatically. `--rule` narrows the active rules by name.
Check reads the graph files again, writes nothing, and accepts normal graph/scope selection.

Node add and edit check the affected node. Edge deletion and a relation- or direction-changing edge edit
check both local ends. No other write triggers rule checks. A batch checks only its final
state, once, so adding a node and its required edge together does not produce a stale warning.
Dry-run batches report the same checks without writing; provisional ids appear as `line:N`.

An unmet shape emits `rule_unmet`, a warning with exit code 0. Its message is the exact fixed
sentence shown by rule list and graph guide. Status, tag, field and regex pattern keys are not
included in that sentence. Its structured `rule` field contains the name,
total count and up to 100 node ids. A single write's warning names its subject and gives that
node's fix. A batch has one warning per unmet rule and a command that lists the unmet nodes.
Descriptions remain untrusted data and never become the warning message or next command.

Checks share the deterministic 100-million-unit matching budget across active rules, with the
same per-node limit described below. An oversized node becomes a `not_checked` row, distinct
from `unmet`, and emits `rule_node_unchecked`; scanning continues. A whole-scan cut emits
`rule_check_cut`. Follow `data.rules[].next` to page the known rows, then resume after the last
visited node with `--after <id>`. The exact resume command is also returned separately.
Exact filters run before budget reservation; a matching type-only rule costs one unit.
When one node's combined checks exceed the entire budget, completed rule results are kept,
unaffordable rules report `not_checked` for that node, and scanning advances in fixed rule-name order.
An incomplete scan has `total: null`; counts describe only the nodes that were checked.
Narrowing or anchoring a regex can reduce its work, but is not required to keep writing.

A field is present when its stored value is not null. Symmetric relations satisfy either
direction. For type-based edge shapes an external far end counts without resolving its type
or loading another graph; an exact `--to-node` target remains local. Provenance nodes are
never rule subjects.

## Patterns

Every given key must match. Entries inside `--type`, `--status` or `--tag` are alternatives, with
comma-separated lists and repeated flags accepted. A tag `origin:*` matches that entire facet.
Tags and facets are free-form; a rule does not require a facet declaration.

`--where <field>=<value>` requires an exact stored field value. JSON values (numbers, booleans,
lists or objects) keep their type; any other value is a string. Quote a JSON string to distinguish
it from a number: `--where 'rating="3"'`. Repeat `--where` for different fields.

`--match <field>=<regex>` searches `title`, `note` or a string field. Repeat it for
up to four different fields. A missing field or a non-string stored value does not match.

    umx rule add high_risk_owned --type risk --status open --where rating=high --edge owns --from person --graph demo
    umx rule add security_owned --type risk,question --match 'title=^SEC-[0-9]+' --edge owns --from person --graph demo --dry-run
    umx rule add origin_owned --tag 'origin:*' --edge owns --from person --graph demo --description "Track an owner for imported work"

The regex dialect is Rust regex with Unicode enabled: no look-around or back-references. A
match can occur anywhere unless anchored, and is case sensitive unless the pattern says `(?i)`.
Each pattern is at most 256 characters and compiles to at most 1 MiB of NFA state, with
an 8 MiB total compiled-state budget per graph. Invalid, unsupported or over-budget
new patterns are refused before a write with `rule_regex_invalid`, a 1-based position and the
reason. Add and load allocate the graph budget in the same rule-name order. Adding an
earlier name may leave a later rule skipped; the reply names that rule, as load does.
Regexes are compiled once per scan, never per node. A lazy DFA uses a cache capped
at 64 KiB per regex and falls back to the PikeVM when needed, including Unicode boundaries.
If a DFA search gives up, its cache is reset before fallback so later nodes can use the DFA.

`--dry-run` reports the rule and one page of the nodes its pattern matches, exactly like show,
and leaves graph.json byte for byte unchanged. Show's `--limit` is 1 to 1000; `--offset` starts
at zero. `rule_matches_cut` names the next page when more nodes match a stored rule. An
unstored dry-run keeps the total but has no next page. Provenance nodes are not part of a
rule's preview.

A preview visits nodes in id order with a deterministic limit of 1 million work units
per node and a 100-million-unit whole-scan backstop. Work is charged only after the
exact filters pass: text bytes for a DFA search, and text bytes times NFA states only
when PikeVM fallback runs. Type-only rules do not read or charge the nodes' note text.
An unaffordable node is counted as not matching this call, not a reason to stop the scan.
Later nodes are still checked. `not_checked` lists the first 100 such node ids and
`not_checked_total` counts all of them. `rule_node_unchecked` names the rule and count;
`rule_scan_cut` explains why `total` is unknown (`null`). Pages still list the matches
that were checked. The rule can still be stored.

## Shapes and vocabulary

Give exactly one shape: `--field <field>`, or `--edge <relation>` with exactly one of
`--from <types>`, `--to <types>` or `--to-node <node>`. An incoming `from` or outgoing
`to` names allowed node types. `--to-node` accepts a full id, a unique prefix of at least six
characters or an address for one existing node in this graph, and stores its full id; it is still
a warn-only shape, not an automatic hub.

    umx rule add risk_register --type risk --edge part_of --to-node <full node id> --graph demo

An undeclared type, status, field or relation does not block adding a rule:
`rule_vocabulary_unknown` names the rule and gives the command that declares each unknown
name. Declare it if that matches your graph design. `--description` is optional
graph text, one line of at most 500 characters, shown only under the untrusted `data` path.
It never appears in an error message or next command.

## Central rules and storage

Std graphs get the central table shipped with umx; a graph without std gets none. This release
ships `std:risk_owned`: a risk should have an owns edge from a person, organisation or team.
An `owner` string does not satisfy it. Use `umx edge add <owner> owns <risk>` to link the owner.
`umx rule off std:<name>` switches a central rule off
for this graph and `umx rule on std:<name>` restores it. Both commands are idempotent.
Names in stored `rules_off` lists that match nothing are retained, not refused.

    umx rule off std:risk_owned --graph demo
    umx rule on std:risk_owned --graph demo

List shows std rules first, marked `source: std`, including switched-off rows marked `off: true`,
then set files in name order, with their stem as source, then graph rules marked `source: graph`.
Every graph or set's `rules_off` contributes to one union, suppressing any rule with that full
name, including another set's or the graph's own. Only own rows and own switch-offs are
stored in graph.json, sorted by name.

The first stored graph rule or central-rule switch-off raises graph.json to format 8.
Switch-offs naming non-std rules require format 10. Older umx refuses an unsupported
format rather than dropping rules. A graph with no rules keeps its format and encoding;
removing the last rule never lowers a format already written. Hand-edited malformed rows
or unparseable regexes are refused at load with the reason. A stored regex that exceeds this
build's size or graph compile budget is skipped, not a reason to refuse the graph:
`rule_skipped` names its rule and gives the command that removes it. The row stays unchanged
on disk and can be removed or re-added with a smaller pattern. If vocabulary a stored rule names is removed or renamed,
the rule still loads and `rule_vocabulary_unknown` gives the command to declare that name.
The same warning accompanies adding an undeclared name. Rules remain warning-only; they never fill a hub.

## Graph-local rule sets

    umx rule set add ./risk_management.json --graph demo
    umx rule set list --graph demo
    umx rule set remove risk_management --graph demo

A set file contains only `rules` and `rules_off`; omitted keys mean empty lists. For example:

```json
{
  "rules": [
    {"name":"rated","match":{"type":["risk"]},"field":"impact"}
  ],
  "rules_off": ["std:risk_owned"]
}
```

The snake_case file stem names the set, never `std`; reserved vocabulary words such as `group`
are allowed as set names. A row named `rated` in this file becomes `risk_management:rated`.
Inspect it with `umx rule show risk_management:rated`, or switch it off for this graph with
`umx rule off risk_management:rated`. Remove the whole copy with `rule set remove`, not `rule remove`.
Turning a graph switch back on cannot undo a switch-off still present in a set.

Add checks every row and regex before copying the exact file content into
`<graph>/rules/<set>.json`. Undeclared vocabulary is stored with `rule_vocabulary_unknown`.
Any `to_node` is a full id of an existing node in the destination graph.
It copies, never links: changing the source or another graph's copy changes nothing here.
Re-adding byte-identical content is unchanged and writes nothing. Different content under
the same stem is refused; remove the old copy, then add again. Adding the first set raises
graph.json to format 10; removal never lowers a written format.

Each set has at most 64 rules and 64 KiB of content. The graph uses at most 32 valid-name
regular set files, selected in filename order. Extra files produce one `rule_sets_cut` warning
with their count and candidate names; invalid-name files do not count toward 32.
At more than 1000 directory entries the whole folder is excluded with one reasoned warning.
Add refuses a copy past either limit without writing.

An unreadable or malformed file produces one `rule_set_skipped` warning naming it and the
reason; none of its rows or switch-offs are used. Other sources load normally. Unknown keys,
including `fill` or `hubs`, invalidate a set. Switch-offs naming no current rule are retained.
List reads the copies fresh; check rereads both the graph and sets. Other reads use the loaded
snapshot and warn `stale_files` after a set changes on disk; reload before writing.
Remove can delete malformed or oversized named copies without reading their bodies.

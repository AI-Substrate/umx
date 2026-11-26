---
id: 226
title: Declare string labels softly and use std risk ratings and links
route: build
status: current
source: PRD 003 rules-22, rules-23, rules-24 (docs only), rules-25
reqs: [rules-22, rules-23, rules-25]
supersedes: none
---
## Do

On a std 1 graph, use risk `likelihood` and `impact` as strings with the ordered labels
`low,medium,high`. Existing std 1 graphs gain them on update, with no adoption. Link ownership from a
person, team or organisation with `owns`, and mitigating nodes to the risk with `mitigates`.

Declare a custom string's labels with `umx ontology add field task priority:string --values low,medium,high`.
Each label is snake_case, at most 64 characters; a field takes at most 32 distinct labels, in declared order.
The list is kept in graph.json and shown by ontology show and the graph guide. Adding labels appends only
the missing ones in the given order; the same add or a subset is unchanged. Appending beyond 32 is
`field_values_invalid`, with a set command as next. An add without values keeps an existing list.

Use `umx ontology set field task priority --values low,medium,high,urgent` to replace a custom list.
`--clear-values` makes a custom field unrestricted. On a std field these commands add to std's labels and
clear only the graph's additions; std's labels stay first. Either may accompany `--role` in one write.

Keep working after `field_value_off_vocabulary`: the string was stored. Its candidates are the declared
labels. For a valid new label, its next adds that one label. For
other strings, its next edits the node with a value placeholder. Matching is exact and case sensitive.

## Don't

Do not declare a risk's owner as text. Prefer std's string ratings; a graph's non-string rating declaration
warns `std_field_conflict_stored` and keeps its kind, on declaration, load and adoption. Fields with those names
on other types are custom, not clashes. Do not treat a
value list as a constraint on node content: it is a nudge, never a fence. Do not put arbitrary prose or
repeats in a declared list, or declare values on a non-string field (`field_values_invalid`); fix its next
and retry. Non-string node values retain the existing `field_kind_mismatch` warning.
Do not expect `mitigates` to reject a non-risk target: it stores the edge with `relation_endpoint_unexpected`.

## Example

```bash
umx node add --type risk --title "Launch slips" --set likelihood=high --set impact=low --graph launch
umx edge add <person> owns <risk> --graph launch
umx edge add <action> mitigates <risk> --graph launch
umx node edit <risk> --set impact=severe --graph launch
umx ontology add field risk impact:string --values severe --graph launch
umx ontology add field task priority:string --values low,medium,high --graph launch
umx ontology set field task priority --clear-values --graph launch
```

## Why

A graph's vocabulary guides its writers without blocking them. Ordered risk ratings line up across graphs;
ownership and mitigation are links to real nodes, not names held as strings. Older builds preserve the
values key through their unknown-field storage, so this needs no graph format bump.

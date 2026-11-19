---
id: 215
title: Tag nodes in a std graph with --tag and filter node list by tags from the tag index
route: build
status: current
source: ws008 t2b-p1 (req-117)
reqs: [req-117]
supersedes: none
---
## Do
In a std graph, give a node tags with `umx node add ... --tag origin:double_diamond --tag needs_review` (or a comma list) and change them with `umx node edit <node> --tag stage:review --untag needs_review`; batch lines take `tags` (node add) and `tag`/`untag` (node edit). A tag is `<value>` or `<facet>:<value>`, each part snake_case. umx stores tags lowercased, deduplicated and sorted, so give them in any order. When a facet's values should be a fixed set, declare them with `umx ontology add tag origin:double_diamond` (or `origin:*` for any value): an undeclared tag in that facet then warns `tag_off_vocabulary`, whose next declares it. Find tagged nodes with `umx node list --tag a --tag b` (both), `--tag a,b` (either), `--tag 'origin:*'` (a facet) and `--no-tag x` (leave out): these come from the graph's tag index, not a scan.

## Don't
Don't write tags that are not snake_case: `origin:Double-Diamond` is refused with `tag_invalid`, and its next repeats the graph, the type and every tag with `origin:double_diamond` instead (for node add the title is a placeholder; other flags are not repeated, and a `--scope` or graph list names its first std graph). Through `--set tags=` or a batch line the next is `umx docs get ontology` instead: there is no command to repeat. Don't give one call both `--set tags=...` and `--tag` (`usage_invalid`). Don't put more than 64 tags on a node (`tag_limit`). Don't expect `--tag` in a graph made with `--no-std`: there `tags` is an ordinary custom field and every tag verb is `tag_needs_std` (its next is `umx ontology adopt-std --dry-run`); a list over several graphs skips such a graph and counts it in `tag_filter_not_std`. Don't put text that should be searched in a tag: tags are data, never embedded or word-indexed.

## Example
```bash
umx ontology add tag origin:double_diamond --graph demo
umx node add --type risk --title "Launch slips" --tag origin:double_diamond --tag needs_review --graph demo
umx node edit 01JC8XA4H7 --tag stage:review --untag needs_review --graph demo
umx node list --tag 'origin:*' --no-tag blocked --graph demo
```

## Why
A tag says something short and exact about a node (where it came from, what it waits on) that types and fields do not, and filters on it must stay cheap as a graph grows. One stored form keeps git diffs and comparisons exact, an index keeps filters from reading every node, and a vocabulary that warns but never refuses lets a team tighten a facet without losing a write.

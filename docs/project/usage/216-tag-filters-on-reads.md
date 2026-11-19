---
id: 216
title: Filter search, node find, walk and neighbours by std tags, and match tags in Cypher
route: explore
status: current
source: ws008 t2b-p2 (req-117)
reqs: [req-117]
supersedes: none
---
## Do
In a std graph, narrow any read to tagged nodes with the flags node list takes: `--tag a --tag b` (both), `--tag a,b` (either), `--tag 'origin:*'` (a facet) and `--no-tag x` (leave out). `umx search` narrows every leg before it ranks, so `--limit 1` gives the best tagged hit; `umx node find` keeps its tagged matches; `umx walk` lists only the tagged nodes it reaches and walks on through the rest; `umx neighbours` lists only the tagged far ends. In `umx query`, write `MATCH (n) WHERE 'needs_review' IN n.tags`: it reads tags in stored form.

## Don't
Don't read `tag_filter_untested` as "no match": it counts rows whose node the read cannot show (an unnamed secured graph, one not loaded, a missing node), left out without reading their tags; name the graph to read them. Don't expect a graph without std to answer a tag filter: read alone it is `tag_needs_std`; in `--graph a,b`, `--scope`, `--all`, or a read that names no graph while several are loaded (a walk or neighbours starting in it too) it is skipped and counted in `tag_filter_not_std`. A refused tag's next repeats the verb, the node (or `<words>`/`<text>` for search and node find: put your words back), the graph and every tag; other flags are not repeated. Over several graphs (a list, a scope, `--all`, an unnamed read) the next names only the first std graph read, so it reads that graph alone. In Cypher, `NOT 'x' IN n.tags` also keeps nodes with no tags (`n.tags` is null there and `NOT` is two-valued); write `n.tags IS NULL OR NOT 'x' IN n.tags` to say "without x" plainly. `'x' IN n.tags` in Cypher reads every node it is asked about; prefer `umx node list --tag x` when that is all you need.

## Example
```bash
umx search approval delay --tag needs_review --limit 5 --graph demo
umx node find "launch" --tag 'origin:*' --graph demo
umx walk 01JC8XA4H7 --depth 3 --tag needs_review --graph demo
umx neighbours 01JC8XA4H7 --no-tag blocked --graph demo
umx query "MATCH (n) WHERE 'needs_review' IN n.tags RETURN n.id, n.title" --graph demo
```

## Why
A tag is only useful if every read can be narrowed by it the same way. Narrowing search before ranking keeps a tagged answer from being crowded off the page by untagged ones, and never testing a row the read cannot show keeps a tag filter from saying anything about a hidden graph.

---
id: 067
title: Search nodes by their words with umx search; find stays for titles
route: explore
status: superseded
superseded_by: 078
source: ws004 s10 (plan 034); workshop 008 section 11; graph-storage-expert and embeddings-expert acceptance (2026-11-10)
reqs: [req-68, req-57]
supersedes: none
---
## Do
Ask "which nodes mention this?" with `umx search <words>`. It matches whole words in the title, the note and every field the node's type declares as a string, and ranks by BM25: rare words count more than common ones, a title match more than a note match. Each hit carries `score` and `field` (the field that earned most of the score).

Search one graph with `--graph demo`, several with `--graph demo,ops`, or every loaded open graph with neither. Secured graphs are searched only when you name them; an unnamed search counts the ones it left out in `secured_graphs_skipped` (a count, never names). Each hit carries its graph's `policy`. Narrow to a subtree with `--within <node>` (an address, `umx:<graph>/<id>`, when several graphs are searched); scores there are the same as in the whole graph.

Declare a custom text field as a string (`umx ontology add field <type> <name>:string`) to make it searchable; it is searchable from that write on.

## Don't
Don't expect typo tolerance or stemming from search: `delay` does not match `delays`, and `aproval` matches nothing. To name one node you half remember by its title, use `umx node find` (typo tolerant), as before. Don't expect a secured graph's nodes from a search that does not name it. Don't search for ids, numbers or dates held in custom fields; filter them with `umx node list --where`. Don't rely on raw score values across different sets of graphs: a word's rarity is judged over the graphs searched, so adding a graph changes scores (not the order within a graph's own exact matches).

## Example
```bash
umx search approval delay --graph demo
# rows: "Approval delay" (field title, highest score), then "Vendor onboarding" (field note)
umx search iso27001 --graph demo,ops --limit 5
```

## Why
Word search finds what a node says, not just what it is called (req-68), so a decision whose note names the cause turns up even when its title does not. The index is rebuilt from the graph files at load and kept by every write, so it is never stale and never committed. Scores are deterministic (same graphs and words, same list on every machine), which is what hybrid search (s11) fuses with the vector leg.

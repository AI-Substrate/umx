---
id: 197
title: Act on a hint that a field is in the wrong role group: labels as data, prose as content
route: build
status: current
source: PRD 002 search-05 (change request 001 c-02, ruling r-01: label fields flood search by meaning and cost an embedder call per node; a prose field left as data is never found by meaning)
reqs: [req-97]
supersedes: none
---
## Do
Declare a string field that holds prose (sentences that differ from node to node) as content, and one that holds labels (a handful of short values repeated across nodes: a tier, a region, a category) as data. When `umx graph guide`, `umx embeddings status` or `umx ontology add field` lists the hint `field_looks_like_labels` or `field_looks_like_prose`, read the field's values, and if the hint is right, run the command in its `next` exactly as printed: `umx ontology set field <type> <field> --role data` for labels, `--role content` for prose. The labels hint says how many values the field holds: each is a field vector and an embedder call that data saves.

## Don't
Don't expect umx to change a role for you: it only points the field out, and the role stays as declared until the command runs. Don't make a field content because it is long once in a while: the prose hint needs more than half its values to be 8 words or more and at least 4 in 5 to differ. Don't look for these hints on number, date, yes/no or list fields, or on fields umx never embeds (provenance and identity keys): they are never pointed out. Don't read node text into a hint: hints carry counts only.

## Example
```bash
umx ontology add field deal tier:string        # content by default
umx graph guide demo                            # hint field_looks_like_labels: tier holds 3 short labels
umx ontology set field deal tier --role data --graph demo
umx ontology add field deal summary:string --role data
umx graph guide demo                            # hint field_looks_like_prose: summary reads as prose
umx ontology set field deal summary --role content --graph demo
```

## Why
On one 536-node graph, a label field with 3 distinct values was content and held 536 of the graph's 2,283 vectors; label fields together held 666 (29%), each an embedder call, and a search for one label returned rows with identical scores matched on that field. The reverse fails quietly: a prose field left as data is still found by words but never by meaning. Numbers in the rule: labels are at least 6 values, at most 12 distinct, each used twice on average, none over 40 characters; prose is at least 3 values, more than half of 8 words or more, at least 4 in 5 different. The project lead ruled that umx hints and never switches a role itself.

---
id: 206
title: Only prose is searched by meaning; a string field with no role is data
route: build
status: current
source: PRD 002 search-04 (change request 001, project lead rulings r-01 and r-04, 2026-11-16); changes the default note 114 gave
reqs: [req-97, req-34, req-35]
supersedes: none
---
## Do
Declare a string field that holds prose (sentences that differ from node to node) as content: `umx ontology add field <type> <name>:string --role content`. Only the title, the note and content fields are embedded and searched by meaning. Leave every other field with no role: a string field with no role is data, like numbers, dates, yes/no and lists. A data string field is still found by words and by `--where` (a data number or date also by exact match), and costs no embedder call.

When a declare (`umx ontology add field`, or `umx ontology add type --field`) gives the hint `field_data_by_default`, the field is data. If it holds prose, run the hint's next as printed (`umx ontology set field <type> <field> --role content --graph <graph>`); if it holds labels, names or ids, do nothing.

In a graph made before this rule, a string field with no recorded role keeps the vectors it has, and they are still searched. Its load warns `text_form_outdated` with how many nodes, and nothing is embedded again or dropped on load. To move the graph:
- A field that should stay searched by meaning: state it content first, `umx ontology set field <type> <field> --role content`. Its vectors are current again, with no embed call (`vectors_pending` may still count its nodes until the graph is next loaded).
- Then run `umx embeddings reembed --dry-run`. `whole_node_texts_changed` is what costs tokens (one whole-node text per node that held a now-data field); `field_vectors_dropped` is the field vectors it removes, which costs nothing. Run `umx embeddings reembed` once the user agrees with that cost.

The std person fields `job_title` and `org` are data: a person node's whole-node text is its title, note and any field you stated content, so `similar_to` between people leans on their names. A graph that declared them `--role content` keeps that.

## Don't
Don't make a label field (a tier, a region, a category, a name) content to have it found: words and `--where` find it, and as content each value is an embedder call that crowds search by meaning. Don't run `umx embeddings reembed` without reading its `--dry-run`, or on another seat's graph without telling its owner first. Don't expect a load to remake or clear the old vectors; and know that editing such a node embeds it again under the rule, so its role-less fields lose their vectors then. Don't expect `field_data_by_default` on a re-declare that changes nothing, on a field you stated a role for, or on a number, date, yes/no or list field.

## Example
```bash
umx ontology add field deal summary:string --role content
umx ontology add field deal region:string          # hint field_data_by_default: region is data
umx search "supplier wants more time to review" --graph demo
umx node list --type deal --where 'region=north' --graph demo
umx embeddings reembed --graph demo --dry-run       # whole_node_texts_changed, field_vectors_dropped
umx embeddings reembed --graph demo
```

## Why
Label fields flood search by meaning with near-identical matches and cost an embedder call per node: on one 536-node graph, label fields held 29% of its vectors. A prose field wrongly left as data fails softly (words still find it); a label wrongly embedded fails loudly. So the project lead made data the default and content a choice. `job_title` and `org` are short labels. An older graph moves only when a person runs reembed, which shows the cost first, so a load never calls the embedder.

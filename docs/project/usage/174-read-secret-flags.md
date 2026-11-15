---
id: 174
title: Read secret_suspected on node, entity and edge reads, and never repeat a flagged value
route: explore
status: current
source: F188, plan 168 (the project manager 2026-11-12; security and project manager rulings on the shape); F190 and F217 (the project manager 2026-11-13; security's ruling on one base); F226 (the project manager 2026-11-13, security's review on #391); F229 (the project manager 2026-11-13, security's review on #393)
reqs: [req-153, req-46]
supersedes: none
---
## Do
Read `secret_suspected` on what `umx node get`, `umx node list`, `umx entity show`, `umx edge show` and `umx edge list` return. It names each shown value umx's own secret check flags, the same check a write warns with: `field:<name>` for the title, note, a string field or `locator`, `key:<i>` for the i-th entry of that object's `entity_key` array, and `evidence:<i>`, `status_reason:<i>`, `status_evidence:<i>` and `evidence_removed:<i>` for those lists, each position counted from 1 (`evidence:1` is the first pointer). On an edge it names `edge:<id>/evidence:<i>` and `edge:<id>/field <n>`, the n-th custom field as the edge's line holds it, counted from 1 (never its name), as `umx check --lint` names those places and a redact's candidates number them. It is left out when nothing is flagged. On `umx node list` and `umx edge list` it covers only the columns each row returns; an edge list row shows no evidence, so `umx edge show` is where an edge's pointers are flagged.

A node's custom field is flagged by its name (`field:zeta`); an edge's by its number (`edge:<id>/field 2`), as the lint and the redact number both. An edge's flags cover only the stored fields `umx edge show` shows: a computed column such as `sentence` hides a stored field of the same name.

A custom field that holds a list or an object is scanned by every string inside it, however deep, an object's keys as well as its values, and a hit is flagged at the field that holds it (`field:creds`, `edge:<id>/field 2`, or an older edge's kept evidence object at `edge:<id>/field:evidence_field_kept`), never at a path inside it. A write that stores such a value warns `secret_suspected` as it does for a string field.

A node's flagged title is named wherever another record shows it, so the same title is never shown unflagged: an edge row (`umx edge show`, `umx edge list`) names `field:from_title`, `field:to_title` and `field:sentence` (the sentence holds both titles); a `umx neighbours`, `umx walk`, `umx path`, `umx search` or `umx node find` row names `field:title`; a `umx similarity pairs` row `field:title` and `field:other_title`. A search row with a flagged title names `field:word_terms` too: each matched term is the node's whole word, so a short prefix query would spell the title's token out. These name the reply's own keys, not a stored place, so they carry no `edge:<id>/` prefix.

Write replies and problems follow the same rule. A write's sentence that holds a flagged title names `field:sentence` on its record: `umx edge add`, `umx edge edit` and `umx edge delete` data, `umx node add`'s `parent_edge` and `raised_in_edge`, `umx entity link` and `umx entity deny` data, and each entry of `umx batch --echo-sentences`'s `sentences`. A `umx graph guide` example names `field:title` (a type's or status's node) or `field:sentence` (a relation's edge). A problem's candidate with a flagged title (a near duplicate, an ambiguous or missing node's matches, a spine's anchors) carries `secret_suspected: ["field:title"]` on the candidate; `--human` prints no candidates.

`umx query` rows name each column whose cell holds a flagged value, as `field:<column>`: a node's title (by the read rule, allows included), any string at any depth (a list's items, a map projection's keys and values), an edge's text-quote selectors and the graph node's title. A node cell with a flagged title also carries `field:title` itself, but the row's flags decide: hide the whole flagged column, never just part of it. A string column ignores allows (it has no node or field to look one up by), so an allowed title still flags as `RETURN n.title`; read it with `umx node get` instead. The check sees the column's name, so `RETURN n.title AS password` can flag where a read would not. A column named `secret_suspected` is refused (`query_column_reserved`).

When a value is flagged, tell the user which node and field hold it and let them decide: redact it with `umx node edit`, or allow it with `umx embeddings allow --node <node> --field <field>` if they know it is not a secret.

## Don't
Don't repeat, quote, summarise or copy a flagged value into a reply, a note, a log or another graph: the reply still holds the value, and the flag only names it. Don't treat a missing `secret_suspected` as proof that nothing is secret: it covers what umx's rules recognise in the values shown, nothing more.

## Example
```bash
umx node get 01JC8XA4H7                          # "secret_suspected": ["field:note", "key:2"]
umx node list --columns id,title,entity_key       # rows carry secret_suspected for title and entity_key only
umx entity show umx:crm/01JC8XA4H7                # members carry ["field:title", "key:1"]; the top level indexes keys
umx edge show 01JC8XE1RM4TQ7W2N5V8B3K6YP          # ["edge:01JC8XE1RM4TQ7W2N5V8B3K6YP/field 2", "edge:01JC8XE1RM4TQ7W2N5V8B3K6YP/evidence:1"]
```

## Why
The graph view and agents need to know which shown values to hide or handle with care without umx echoing the value a second time. Names are already visible as object keys, so naming a flagged field or a position in a list leaks nothing new. One check for writes and reads means a value a write warned about is the value a read flags.

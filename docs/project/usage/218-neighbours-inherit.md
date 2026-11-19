---
id: 218
title: Neighbours lists the edges of the node's entity members, with via and match_status
route: explore
status: current
source: ws008 E3 (req-127)
reqs: [req-127]
supersedes: none
---
## Do
Run `umx neighbours <node>` on a node that is one entity with nodes in other graphs (a shared `entity_key`, an `umx entity link`, or a `possibly_same_as` proposal): after the node's own rows it lists each other member's stored edges. Every row of such a reply has `match_status` (`own`, `confirmed` or `proposed`) and `via`, the address of the member an inherited row came from; `from` and `to` stay the stored ends. The list's `graphs` names the graphs the read covered, and the `entities_grouped` hint says how many nodes were grouped and how many through proposals, with `umx entity show` as its next to list the members. Use `--entities confirmed` to leave proposals out, and `--entities off` for the node's own edges only. Name a secured graph (`--graph a,b` or `--scope`) to include its members; pin the covered graphs with `--scope`.

## Don't
Don't expect the old rows by default: this changes `umx neighbours` for every node that has a key or link shared with another loaded node. Pass `--entities off` when a script needs only the node's own edges; that reply is byte for byte what neighbours gave before. Don't read an inherited row as the node's own edge: it is the member's (`via`), and nothing merged is stored. Don't look for identity edges (`same_as`, `possibly_same_as`, `not_same_as`), edges between members, `similar_to` or `--provenance` rows among the inherited rows: they are never inherited. A proposal is a `possibly_same_as` partner of the node or of any confirmed member (as `umx entity show` lists it) and brings only its own edges, never the partner's other members. Under `--entities confirmed` a proposal partner is not a member, so a member's edge to it is listed as an ordinary inherited row (only the `possibly_same_as` edge itself is left out). Don't expect neighbours to repeat entity show's warnings (two types holding a key, a link skipped because of a denial): run the hint's next, `umx entity show`. An address in one graph with `--graph` naming another is refused (`graph_mismatch`) unless you pass `--entities off`. A secured graph the read does not name gives nothing, even when a link through it joins two open members; two copies of one graph never group.

## Example
```bash
umx neighbours umx:crm/01JC8XA4H7
umx neighbours umx:crm/01JC8XA4H7 --entities confirmed --relation works_for
umx neighbours umx:crm/01JC8XA4H7 --entities off
umx neighbours umx:crm/01JC8XA4H7 --scope people
```

## Why
One thing recorded in several graphs should answer "what does it connect to" in one read, without a merged copy anyone has to keep in step. Labelling each row with where it came from (`via`) and how sure the join is (`match_status`) keeps the answer honest, and building it from the graphs the read covers keeps a secured graph's content out of reads that did not name it.

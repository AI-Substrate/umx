---
id: 150
title: Join one thing's nodes with umx entity link; see the whole entity with umx entity show
route: multi-graph
status: current
source: ws008 E2; D63 and D63c Q4, Q8 and Q9 (decisions, 2026-11-11), an agent's entity identity design
reqs: [req-127]
supersedes: none
---
## Do
When two existing nodes, usually in two graphs, are the same person, organisation or thing but hold no shared `entity_key`, join them with `umx entity link <a> <b>`. It stores one `same_as` edge (a std symmetric relation, stored once) in the graph `--graph` names, or else in the first node's graph. It writes no key. Linking again is `"unchanged": true`.

To see an entity, run `umx entity show <node>`. It lists every member by graph and how each joined (`matched`: a shared `entity_key`, or a `same_as` link), plus nodes a `possibly_same_as` edge proposes (`proposed`, never merged). Membership is transitive: a key match and a link chain together. The reply's `graphs` lists what it covered: every loaded open graph plus the node's own, or exactly `--scope` / `--graph a,b`, which must include the node's graph. A secured graph counts only when the read names it.

Fix a wrong match at its source: `umx entity unlink <a> <b>` removes the link (from `--graph`, else either node's graph), or `umx entity key <node> remove <key>` removes a key.

## Don't
Don't link two nodes because their titles look alike: add a `possibly_same_as` edge and let a person confirm it. Don't link a node to itself or two copies of one graph (`entity_link_invalid`). Don't ignore `entity_type_mismatch`: the link is stored, but check the two types are meant to be one entity. Don't expect `entity show` to name a node in a secured graph you did not name, or in a graph that is not loaded.

## Example
```bash
umx entity link umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M
umx entity show umx:crm/01JC8XA4H7
umx entity show umx:crm/01JC8XA4H7 --scope people
umx entity unlink umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M
```

## Why
Some things never share a key: two graphs recorded the same person before either had an email for them, or an organisation is keyed by domain in one graph and ABN in another. A link joins them without touching either node, and it can be listed and removed. Nothing merged is stored, so `entity show` builds the entity when asked, from the keys and links in the graphs it covered, and says which graphs those were.

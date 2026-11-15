---
id: 172
title: Read entity guard warnings, use the entity address, and look keys up across open graphs
route: multi-graph
status: current
source: ws008 E2b, plan 138 (the project manager 2026-11-12; the architecture reviewer's entity review; the graph-addressing expert's F4 ruling; D69)
reqs: [req-127]
supersedes: none
---
## Do
Read `umx entity show`'s warnings before you trust an entity. `entity_graph_twice` means the entity holds two nodes of one graph: a key or a link probably joined two different things; it names the graph, the two nodes and what joined them (a key's scheme or the link's id). `entity_key_ignored` means a key held by several nodes of one type in a graph joined none of them there. `entity_types_mixed` means the members hold more than one type. `entity_type_mismatch` means the same key sits on two types (an organisation in one graph, a team in another), so those nodes never match; its next is the retype, `umx node edit <node> --type <type>`.

Use the reply's `entity` (the lead member's address: lowest graph id, then node id, among the members shown) or `entity_id` (the same member as `umx:<graph id>/<node id>`, stable when aliases change) to name the entity in later commands. Both depend on which graphs the read covered.

Look a key up with `umx node list --where entity_key=<key>`: with no graph named and several loaded, it covers every loaded open graph.

Check a link before you store it with `umx entity link <a> <b> --dry-run`: it gives every refusal and warning a real link would, and writes nothing. A batch `edge add` line of type `same_as` between two types warns `entity_type_mismatch` too.

When `umx entity link` warns `entity_link_overlap`, the link is stored but joined an entity that already had a node in the other end's graph: check it with the `umx entity show` its next gives.

## Don't
Don't give a node its own title as a key unless the thing really is known only by that name (`entity_key_is_title` hints). Don't give two different things in one graph the same key: it then joins neither. Don't assume `entity` is fixed: loading another graph can change the lead. Don't write `domain:` or `url:` hosts in mixed case expecting them to differ: they are stored lowercase.

## Example
```bash
umx entity show umx:crm/01JC8XA4H7            # warnings, then "entity": "umx:billing/01JB...", "keys": [...]
umx entity show umx:01JB2C3D4E5F6G7H8J9K0M1N2P/01JB...   # the entity_id works too
umx node list --where entity_key=domain:example.com   # every loaded open graph
umx node edit umx:people/01JD2B9Q0M --type organisation   # the entity_type_mismatch next
```

## Why
An entity is built when you ask, from keys and links, so a bad key or link silently joins different things or silently misses the same thing. The warnings make those visible at read and write time without refusing anything (E1's rule: warn on doubtful input). The entity's address is a member's address, so every command that takes a node takes it, and no separate id is stored.

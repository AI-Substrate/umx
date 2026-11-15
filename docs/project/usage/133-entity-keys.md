---
id: 133
title: Give one thing the same entity_key in every graph; look it up before adding it
route: multi-graph
status: current
source: ws008 E1; D63 and D63c (decisions, 2026-11-11), an agent's entity identity design
reqs: [req-102]
supersedes: none
---
## Do
When a person, organisation, team, system or anything else appears in more than one graph, give each of its nodes the same key in the std field `entity_key`: a list of `scheme:value` keys. Two nodes are the same entity when they have the same type and share a key.

Use a natural key where one exists: `email:` or `upn:` for a person (allowed; compared lowercase and trimmed), `domain:` or `abn:` for an organisation. With no natural key, use `id:<umx id>`, the id of the node that first recorded the thing.

Keys compare exactly, except `email:`, `upn:` and `domain:` (lowercased), `url:` (scheme and host lowercased) and `id:` (canonical id text): `name:Acme` and `name:acme` are two different keys. So write a natural key in lower case wherever its source is case-insensitive. A value is at most 256 bytes with no control, bidi or zero-width characters, `id:`, `email:` and `upn:` values hold no spaces, and a node holds at most 32 keys; anything else is refused with `entity_key_invalid`.

Before adding the thing to another graph, look it up across the graphs you have: `umx node list --where entity_key=<key> --graph a,b` (or `--scope <name>`). If it is there, copy its key onto the new node. That is how keys stay the same without a central graph.

Set keys with `umx entity key <node> add|remove <key>`, or as a field: `--set entity_key=email:a@x.com,upn:a@corp` on `node add` and `node edit`, and a batch line's `sets`. Every key is checked; a malformed one refuses the write with `entity_key_invalid` and nothing changes.

## Don't
Don't key two nodes the same because their titles look alike: a shared key says "this is the same thing". Don't use a shared mailbox or role address as a person's key: everyone listing it becomes one entity. Don't declare `entity_key` with `ontology add field` (`reserved_name`): its kind is fixed. Don't expect keys in search or embeddings: they are data only. Don't leave a `duplicate_key` (two nodes of one type in one graph with the same key): remove the key from the node that should not have it.

## Example
```bash
umx node list --type person --where entity_key=email:jo.smith@example.com --graph crm,meetings
umx node add --type person --title "Jo Smith" --set entity_key=email:jo.smith@example.com
umx entity key 01JC8XA4H7 add upn:jsmith@corp.example
umx entity key 01JC8XA4H7 remove email:old@example.com
```

## Why
D63: no central graph; an entity is its nodes in their own graphs, joined by a key they share. Merging their edges at read time comes in later steps (ws008 E2, E3) and builds on these keys, so keys set now count then. Email and UPN keys were held back by the interim std sheet; D63c Q2 lifted that, because whoever holds a graph is trusted with it.

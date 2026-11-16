---
id: 202
title: Add with on_key resolve so a node found by its entity key is enriched, not duplicated
route: build
status: current
source: ws003 s08; graph-addressing and graph-storage design verdicts (2026-11-17)
reqs: [req-88, req-102]
supersedes: none
---
## Do

When you add things you may have added before (a person, an organisation, a system, met again in the next
transcript window), give each its `entity_key` (usage 133) and opt in to resolve by key: `"on_key":"resolve"` on a
batch `node add` line, or `--on-key resolve` on `umx node add`. The add looks each of its keys up among the nodes of
its own type in the graph it writes, as the batch has left it so far. No node holds them: the node is added as
usual. One node holds them: the add resolves to that node instead.

A resolved add only appends or fills. New evidence is appended (a pointer the node holds already is not added
again). A field the node lacks is filled, an empty note is filled, keys the node lacks are added. A `parent` is
added only when the node has no `part_of` edge yet; a `raised_in` anchor (the line's or the batch's `--raised-in`)
adds its edge unless the node has that edge already. Parents and anchors compare by the node they name, so an id
and its `umx:<graph>/<id>` address are the same. A resolve with nothing new writes nothing.

Read the row: `existed` is true for a resolved line, `id` is the node it found and `changed` lists what the line
added (`note`, filled field names, `entity_key`, `evidence`). `parent_edge` and `raised_in_edge` name an edge only
when the line added one. A resolved line's `ref` binds to the found node, so later lines, the receipt and
`--refs-from` work the same either way. The single verb's reply carries `resolved` and `changed` beside `node`.
Dry-run first: it gives the same rows (a node an earlier line of the dry run adds still has a null id).

Two lines of one batch with one key: give the later one `on_key` and it resolves to the node the earlier one adds.

## Don't

Don't expect a resolve to change anything the node holds: the title, type and status never change, and a note or
field with another value is kept. The `on_key_field_held` hint names what was kept, never the values, and its next
is the command to change the first one yourself (`umx node edit`, `umx node status`, `umx node note`). A node with
another parent keeps it, and the hint names `parent`.

Don't count on a key matching across types or graphs: the lookup is per type, in the written graph only. The same
key on a node of another type adds a new node and warns `entity_type_mismatch` (its next retypes the new node).
Other graphs, copies included, are never read, and a `same_as` link never makes a resolve reach another graph.

Don't leave a key on two nodes of one type (`duplicate_key`): an add with that key, or with keys held by two
different nodes, is refused with `on_key_ambiguous`, which names both nodes by address and writes nothing. Its
next removes the key from one of them (`umx entity key <node> remove <key>`). umx never picks the lower id.

Don't give `on_key` with an explicit `id` (`batch_line_invalid`: ids are for umx-to-umx ports) or without an
`entity_key` (`batch_line_invalid`, or `usage_invalid` from the single verb), and don't give `--on-key` with
`--locator`: a resource node is already found by its locator. A batch line's `created` applies only when it adds
the node.

Don't expect a status to hide a node: a node marked closed or deleted still holds its keys; remove its key to stop
it matching. A kept line (an earlier line of a repeated id), an entry umx cannot read and a provenance node never
match.

## Example

```sh
cat > window-08.jsonl <<'OPS'
{"op":"node add","ref":"jo","type":"person","title":"Jo Smith","on_key":"resolve","sets":[{"field":"entity_key","value":"email:jo@example.com"}],"evidence":["calls/8.vtt#seq=4-9"]}
{"op":"node add","ref":"acme","type":"organisation","title":"Acme","on_key":"resolve","sets":[{"field":"entity_key","value":"domain:example.com"}]}
{"op":"edge add","from_ref":"jo","relation":"works_for","to_ref":"acme"}
OPS
umx batch --file window-08.jsonl --graph launch --raised-in umx:launch/01JC8XA4H7 --dry-run
umx batch --file window-08.jsonl --graph launch --raised-in umx:launch/01JC8XA4H7
umx node add --graph launch --type person --title "Jo Smith" --set entity_key=email:jo@example.com --on-key resolve
```

## Why

Agents ingesting in windows kept their own map from names to ids between batches, or added the same person again
in every window. A key the caller chooses, in the one key grammar umx already has (`entity_key`, the same index that
finds `duplicate_key`), lets the graph be that map. Append-or-fill keeps a resolve from rewriting what earlier
evidence established, and refusing an ambiguous key keeps umx from guessing which node was meant.

---
id: 065
title: Ask how two nodes connect with umx path
route: explore
status: current
source: req-28; workstream 002 s11; plan 032
reqs: [req-28]
supersedes: none
---
## Do
Run `umx path <from> <to>` to get a shortest chain of edges between two nodes: the nodes in order, each after the first with the `relation`, `way` and `edge` that led to it. By default the path may follow edges either way and is at most 6 edges long; use `--direction out` to keep to edge direction, `--max-depth N` to look further or nearer, `--relation <r>` for one relation.

## Don't
Don't treat an empty result as an error or as proof the nodes are unrelated: it means no path within the depth (and `similar_to` edges are not followed unless named). Don't assume the path is the only one; when several are equally short you get the first found.

## Example
```bash
umx path 01JC8XA4H7 01JC8XB2KQ
umx path 01JC8XA4H7 01JC8XB2KQ --direction out --max-depth 3
```

## Why
Explaining a connection ("why does vendor lock in matter to the launch") needs the edges named, not just a yes.

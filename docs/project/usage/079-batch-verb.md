---
id: 079
title: Make many changes at once with umx batch
route: build
status: superseded
superseded_by: 104
source: decision D48 (2026-11-10); workstream 003 s07, plan 044 (shipped)
reqs: [req-88]
supersedes: none
---
## Do
When you add or change more than a handful of nodes or edges, write one JSON op per line to a file (or stdin)
and apply them with one call. Give a node a `ref` when a later line needs to point at it, and point at it with the `_ref` twin of the field (`from_ref`, `to_ref`, `parent_ref`, `node_ref`); a real node is named with the plain field (`from`, `to`, `parent`, `node`). A ref must be defined on an earlier line and exists only inside the batch.
Run it with `--dry-run` first when the batch is large or generated: it checks every line exactly as the real run
would, then writes nothing and draws no ids (every new `id` is null).
A ref is a lower-case letter then up to 31 of `a-z 0-9 _ -`, and may not read as an id prefix (six or more
Crockford characters, such as `answer`): put a `_` or `-` in it (`q1`, `answer_1`). A set may leave out `as`: a
string value reads as `--set` reads it, any other JSON value is taken as is.

## Don't
Don't loop over `umx node add` for bulk work: each call rewrites the graph's files.
Don't build long command lines or shell-quote JSON into arguments; put the ops in a file or pipe them.
Don't put a ref in a plain field or an id in a `_ref` field; each line gives exactly one of the pair.
Don't expect part of a batch to land: if any line fails, nothing changes and the error names the line.
Don't rerun a single verb to fix a failed line (the `next` says `umx batch --file <file> --dry-run`): fix the line
in the file and rerun the batch.
Don't send more than 10,000 ops or 8 MiB in one batch (`batch_too_large`); split a large import.
Don't expect titles back: the reply names nodes and near-duplicate candidates by id only (check an edge's
direction with `umx edge list --from <node>`), and lists at most 100 warnings.

## Example
```
cat > ops.jsonl <<'OPS'
{"op":"node add","ref":"p1","type":"problem","title":"Approval delays"}
{"op":"node add","ref":"o1","type":"option","title":"Hire a second approver"}
{"op":"edge add","from_ref":"o1","relation":"supports","to_ref":"p1"}
{"op":"node edit","node_ref":"p1","sets":[{"field":"likelihood","value":3}]}
OPS
umx batch --file ops.jsonl --dry-run
umx batch --file ops.jsonl
```
The result maps each ref to its new id, with one row per op line (`--human` shown):
```
line  op         ref   id                          address                              parent_edge  changed         existed
1     node add   p1    01M72MWAAVBJHFNVP0HBWHMADF  umx:demo/01M72MWAAVBJHFNVP0HBWHMADF  none         []              false
2     node add   o1    01M72MWAAVBJHFNVP0HBWHMADG  umx:demo/01M72MWAAVBJHFNVP0HBWHMADG  none         []              false
3     edge add   none  01M72MWAAVBJHFNVP0HBWHMADH  umx:demo/01M72MWAAVBJHFNVP0HBWHMADH  none         []              false
4     node edit  none  01M72MWAAVBJHFNVP0HBWHMADF  umx:demo/01M72MWAAVBJHFNVP0HBWHMADF  none         ["likelihood"]  false
```
A file with a bad line changes nothing and names each bad line:
```
error: The batch was not applied: 2 of its lines failed. Nothing was written. [batch_failed]
  next: umx batch --file <file> --dry-run
error: line 1: (that name) is not allowed as a node type: ... [name_invalid]
  next: umx batch --file <file> --dry-run
error: Line 2 names a ref that no earlier line defines. [batch_ref_unknown]
  next: umx docs get batch
```

## Why
A batch writes each graph file once, so 1,000 adds cost about one write instead of 1,000, and all or nothing
keeps the graph consistent if something in the middle is wrong.

---
id: 104
title: Batch changes and ports can keep umx ids
route: build
status: superseded
superseded_by: 136
source: project manager batch-ids packet (2026-11-11); plan 072; architect and graph experts
reqs: [req-88, req-67]
supersedes: 079
---
## Do

For more than a handful of adds or edits, write one JSON op per line to a file or stdin. Run
`umx batch --file ops.jsonl --dry-run`, then `umx batch --file ops.jsonl`. Every line applies or none
does, each file is written once, and embedding is queued after the write.

Carry a full `id` on node and edge adds only for umx-to-umx ports. For a non-ULID source, let umx
mint the id and keep the old one in a custom field such as `source_id`. The rows and ref map return
the new ids so the port can rewrite its links. Node adds can carry `created` as a timestamp,
normalized to UTC; otherwise it is now. An id's time and `created` may differ for ported nodes.
Ids are opaque: never read a time from an id.

Give a node a `ref` when a later line points at it. Use the `_ref` twin (`parent_ref`, `from_ref`,
`to_ref`, `node_ref`) for a ref and the plain field for an id, prefix or address. Each pair is
given exactly once, and refs are defined before use. A ref on an explicit-id add binds to that id;
later lines may use either. Refs live only in the batch, never in the files. A ref is a lower-case
letter then up to 31 of `a-z 0-9 _ -`. Ordinary words such as `estate`, `contract`, `workshop`
and `answer` are valid refs. The separate `_ref` fields keep refs unambiguous; never infer an id
from a ref's spelling.

On disk an edge's relation is stored as `type` (the record's kind, as for nodes and as Cypher's type(r)); the CLI and JSON say `relation`.
Batch edge input accepts either spelling, but not both on one line. The file key never changes.
A set's string value reads as `--set` reads it; other JSON values are taken as is. Use `"as":"str"`
to keep a string as text.

## Don't

Do not loop over `umx node add`, build long command lines, or shell-quote JSON into arguments.
Do not convert UUIDs or other source ids, use id prefixes as new ids, or derive ids from hashes.
Invalid explicit ids are `id_invalid`. Ids already held live, tombstoned, in kept lines, or on
another batch add are `id_taken`. Ids more than 60 seconds ahead of now are `id_in_future`.
Keep source ids in `source_id` instead, or correct the source clock.

Do not assume copied node identities are harmless: `id_in_other_graph` warns that a different
loaded graph id holds the same node id, but only when that graph is Open or explicitly named by
this request. Other secured graphs are skipped silently, without a warning or count. Copies of the
same graph id use d-21 instead. Unloaded graphs cannot be checked. Do not expect `umx node add --id`.

Do not put refs in plain fields or ids in `_ref` fields. If any line fails, fix the named lines
in the file and rerun the batch, not a single verb. Nothing is written after a line error or
failed write. Do not send over 10,000 ops or 8 MiB; lines are at most 1 MiB. Over-limit input
is refused. Blank and comment lines count toward the reported source line.

Do not expect titles back. Rows and near-duplicate candidates carry ids only; inspect an edge
with `umx edge list --from <node>`. At most 100 warnings or line errors are listed, with a
truncation code giving the total. Dry runs draw no ids and write nothing: explicit ids stay
visible and minted ids are null. A passing dry run assumes the graph has not changed before
the real run.

## Example

```sh
cat > ops.jsonl <<'OPS'
{"op":"node add","id":"01JC8XA4H7B3KT9M2NZ5QW6RDE","created":"2024-11-11T00:00:00Z","ref":"p1","type":"problem","title":"Approval delays"}
{"op":"node add","ref":"o1","type":"option","title":"Hire another approver","sets":[{"field":"source_id","as":"str","value":"legacy-42"}]}
{"op":"edge add","id":"01JC8XA4H7B3KT9M2NZ5QW6RDF","from_ref":"o1","type":"supports","to_ref":"p1"}
{"op":"node edit","node":"01JC8XA4H7B3KT9M2NZ5QW6RDE","sets":[{"field":"likelihood","value":3}]}
OPS
umx batch --file ops.jsonl --dry-run
umx batch --file ops.jsonl
```

## Why

Bulk ports need stable node and edge addresses, without changing file keys or losing kept history.
Full-id validation prevents accidental reuse. One bulk ordering pass avoids a rebuild for every
old id. Atomic writes and exact rollback keep a bad line from leaving half a port behind.

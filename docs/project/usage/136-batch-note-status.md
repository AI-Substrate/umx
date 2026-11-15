---
id: 136
title: Batch note append, mixed-kind evidence and status moves with scan parity
route: build
status: superseded
source: ws003 s10; plan 107; architect and security decisions (2026-11-11)
reqs: [req-88, req-58, req-12, req-49, req-47, req-67]
supersedes: 104
superseded_by: 147
---
## Do

For bulk work, write JSON op lines to a file or stdin. Run `umx batch --file ops.jsonl --dry-run`,
then the real batch. Adds, edges, edits, note appends and status moves commit together, writing each
touched file once. Embedding is queued only after a successful real write.

Use `node note` to append a paragraph and evidence; `node edit` with `note` replaces the whole note.
Each note evidence entry has a `pointer` and optional `kind`: `room`, `chat`, `screen`, `document`
or `analyst`. An omitted kind means `document`. Mix kinds in one line; do not give an op-wide kind.
There are at most 32 pointers per note line. Use `node status` with `status`, optional `reason`
and optional `label`; it records the same lifecycle history and warnings as the single verb.

Carry full 26-character umx ULIDs on adds only for umx-to-umx ports. For other source ids, let umx
mint ids and store the old id in `source_id`. Node adds can carry a `created` timestamp, otherwise
now. Ids are opaque: never derive time from an id. A ported id and `created` can have different times.

Use plain fields for ids, prefixes or addresses and `_ref` twins for refs from earlier add lines.
Refs are lower-case letters followed by up to 31 of `a-z 0-9 _ -`; ordinary words are fine.
Refs are batch-local, defined once before use. An explicit-id add binds its ref to that id.
Edge input accepts exactly one of `relation` or `type`; disk edges retain the `type` key.

Dry runs use the same validation and scan as real runs, but write nothing and draw no minted ids.
Explicit ids remain visible; minted ids are null. Replies give bounded ids/change metadata, never
note bodies, pointer text, status reasons or titles. Read nodes/edges back to inspect their content.

## Don't

Do not loop over single writes or build long shell arguments. Do not convert UUIDs into umx ids,
reuse live/tombstoned/kept ids, or put ids in `_ref` fields. `id_invalid`, `id_taken` and `id_in_future`
refuse a bad port. More than 60 seconds ahead is a future id. `id_in_other_graph` checks only open
or explicitly named graphs with another graph id; hidden graphs stay silent and copies use d-21.

Do not expect a passing dry run to bypass the scan. A hit fails dry and real batches at its line,
including on old add/edit ops. It cannot be hidden by a later cleanup line. Nothing is written
or queued after a line error or failed write. A real storage I/O failure is still possible.

The write scan covers note/title/content strings, status reasons, evidence selector/quote and URL
query/fragment text after percent-decoding, and edge custom strings, including fields an edge edit
writes back. It keeps path-like filename
exemptions and fetches no URL. Embedded fields have the existing
`umx embeddings allow --node <id> --field <field>` override. The new status/evidence/edge metadata
surfaces have **no allow yet**; a generic metadata allow is a later follow-up.

Single verbs warn `secret_suspected` and store; a batch refuses. Remove or replace a flagged value.
Only after a person has checked it is not a secret may that one change be made as a single verb.
Do not automatically retry a refusal as a single write. The safe next is `umx docs get batch`,
not a replay of the failed line. The embedding hold keeps its existing inputs.

Do not send more than 10,000 ops, 8 MiB total or 1 MiB per line. Blank/comment lines count in error
positions. At most 100 warnings or line errors are reported, with a truncation total. A changed
graph, config, allow or root state can invalidate earlier dry-run validation.

## Example

```sh
cat > ops.jsonl <<'OPS'
{"op":"node add","ref":"p1","type":"problem","title":"Approval delays"}
{"op":"node note","node_ref":"p1","append":"Reviewed with finance","evidence":[{"pointer":"notes.md#L1-L4","kind":"document"},{"pointer":"analysis.md#quote=The cause is waiting","kind":"analyst"}]}
{"op":"node status","node_ref":"p1","status":"agreed","reason":"Evidence reviewed","label":"resolved"}
OPS
umx batch --file ops.jsonl --dry-run
umx batch --file ops.jsonl
```

## Why

Batch work should not fall back to hundreds of single calls for notes, evidence or status changes.
Shared mutation helpers preserve single-verb semantics, and pre-commit scan parity makes dry runs
useful without weakening the secret guard or widening embedding inputs.

---
id: 135
title: Put a relationship's source on the edge with --evidence, not in a why field
route: build
status: current
source: ws006 S5b (plan 106), graph-addressing-expert topic:edge-evidence (2026-11-11), graph-storage-expert layout ruling (2026-11-11)
reqs: [req-124]
supersedes: none
---
## Do
When you know where a relationship was asserted (a transcript line, a file range, a document), give it as evidence
on the edge: `umx edge add <a> feeds <b> --evidence 'root:repo/src/x.rs#L10-L20' --evidence-kind document`. Pointers
use the same grammar and checks as node evidence (usage 105 for `umx:` sources). Re-running `edge add` for an edge
that exists with a new pointer appends it (`evidence_appended`); nothing else changes. Read it back with
`umx edge show <edge id>`; `umx edge list` shows `evidence_count` and `evidence_kinds` per edge.

## Don't
Don't write a relationship's source into a custom field such as `why` or `evidence`: `evidence` and
`evidence_kinds` are reserved edge field names, and an older edge's own `evidence` field that is not an evidence list
is kept but never read as evidence (`evidence_field_kept`), and evidence given to such an edge is refused (`evidence_field_invalid`) until that
field is moved (`umx edge edit <edge> --set-json why=<value> --unset evidence`). Don't use edge evidence to say which passage an end
is: that is a selector edge (usage 014). Evidence never changes which edge it is.

## Example
```bash
umx edge add 01JC8XA4H7 feeds 01JC8XB2KQ --evidence 'root:repo/src/x.rs#L10-L20' --evidence-kind document
umx edge add 01JC8XA4H7 feeds 01JC8XB2KQ --evidence 'transcripts/016.ndjson#seq=3206-3240' --evidence-kind chat
# evidence_appended: 1 new pointer
umx edge show 01JC8XE1RM4TQ7W2N5V8B3K6YP
```

## Why
Node evidence says where a node's content came from; edge evidence says where a relationship was asserted; a
selector end says the relationship is to a passage. Keeping the source on the edge lets `edge show`, `edge list` and
later checks find it, instead of leaving it as prose in a field nothing reads.

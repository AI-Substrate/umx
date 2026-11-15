---
id: 091
title: A field that looks like a secret is held from hosted embedders until you redact or allow it
route: ingest
status: current
source: workstream 004 s08 (plan 058); rule set and allow shape agreed with security and architect 2026-11-11; file names and paths not flagged, ws004 s23 (plan 083), agreed with security 2026-11-11
reqs: [req-49]
supersedes: none
---
## Do
When a write (`node add`, `node edit`, `node note`, `umx batch`) warns `secret_suspected`, look at the named node and
field. If the text really holds a credential, redact it with `umx node edit <node> --note "..."` (or `--set` for a
custom field) and keep the secret out of the graph. If it is not a secret (a documented example, a test value) or may
go to the configured provider, allow that exact text with `umx embeddings allow --node <node> --field <field>`.

The write itself always happens. Only hosted embedding of that field, and of the whole-node vector that contains it,
waits; local embedders embed it as usual. Until then the node counts in `vectors_held` (not `vectors_pending`) in
`umx graph list`, and with no whole-node vector it drops out of whole-node similarity: `umx search` finds it by its
words only, and it gets no `similar_to` edges.

File names and paths (words, dates and versions joined by `-` or `_`, such as `RECAP-GLOBEX-roadmaps-2026-03-14.md`)
are not flagged as random tokens, so a document path field needs no allow. If one still warns, it holds a
random-looking part or a named key (`sk-...`, `ghp_...`): check it before allowing.

## Don't
Don't paste API keys, tokens, private keys or passwords into node text to "keep them with the concept". Don't allow
a field to make the warning go away: the allow sends that text to the hosted provider. Don't expect an allow to
follow an edit: it covers the current text only, and a changed text is held (and warned) again. Don't expect it on
another machine: allows live in this machine's `$UMX_HOME/allows/secret-allows.json`, at most 256 (and 60 KiB). When
it is full the allow is refused with `secret_allow_refused`: remove allows by redacting or changing their texts.

## Example
```bash
umx node add --type problem --title "Export fails" --note "the job uses <key> for the bucket"
# warning secret_suspected: Node 01K... field note in graph demo looks like it holds a secret (aws_access_key) ...
umx node edit 01K... --note "the job uses the bucket key from the vault"
# or, when it is safe to send:
umx embeddings allow --node 01K... --field note
```

## Why
req-49 (SEC-10): text an agent ingests can carry credentials, and a hosted embedder would send them off the machine.
The hold is decided from the text right before every hosted send, so it survives restarts and hand edits with
nothing stored; only the explicit allow is stored, by the text's hash, never the text.

---
id: 219
title: Redact a sensitive evidence pointer by its source; a common section heading never blocks it
route: build
status: current
source: F183 (security's ruling on scope, trail shape, refusals and the reply); dogfood reports 2026-11-24 that a section heading blocked a URL's redact
reqs: [req-126]
supersedes: 175
---
## Do
Use `umx node evidence <node> remove '<pointer>' --redact --reason "<why>"` only when the pointer itself must not stay in the graph, for example a path or quote that holds a credential, a private name or client text. Copy the pointer from `umx node evidence <node> list` or `list --removed`. The node must hold it, live, in its trail or cited by one of its status moves (`node get` shows those in `status_history`).

One redact covers the whole graph. Every pointer in this graph with the same source is removed: on every node, live, already in a trail or cited by a status move, and in edge evidence. The reply gives only counts: `redacted.pointers`, `redacted.nodes` and `redacted.edges`. Each redacted pointer becomes a trail entry with `kind`, `redacted: true`, `removed_at`, `by`, `by_session` and your reason. The source, selector and hash are gone, and `list --removed` shows `(redacted)`. A status move that cited it stays; its `evidence` becomes a redacted entry of the same shape in place.

Use it also for a pointer a plain remove already put in the trail: copy it from `list --removed` and redact it; its trail entry becomes `(redacted)` and nothing else on the node changes.

What is checked is the pointer's source and its selector's quoted text (`quote=`, `prefix=`, `suffix=`, and any key umx does not know). A `section=` heading is document structure: it is checked only when it looks like a secret, so a common heading such as `section=Experience` used elsewhere in the graph does not block. Line, seq and page numbers are never checked.

If it is refused with `evidence_redact_blocked`, the pointer's source or quoted text would still be somewhere after the redact: any value or field name of a node or edge (a title, note, field, type, created_by, locator, status reason, another pointer or trail entry), the kind or author a redacted entry keeps, a repeated line, or graph.json as it is on disk. A node the redact does not change is read as its line is stored, so a field a hand edit wrote twice in one line counts, and edges.jsonl is always written again from its edges. The session and agent records the redact itself writes count too: if your writer name or role holds the text, redact as another writer. Escaped spellings and other cases count (Unicode case folding: `ß` and `ss` are one). Each candidate names the node or edge id (or the file) and a place: a standard field's name, a list position, or a custom field's number (`field 3`). It never gives the text or a custom field's name. The refusal's next opens the first record named (`umx node get`, `umx edge show`, or `umx check` for a file). Edit each place with `umx node edit`, `umx edge edit` or `umx graph set`. For a repeated line, run `umx check` and keep one line per id. For a field written twice in one line, edit that line in nodes.jsonl so it holds the field once. Then redact again.

The source matches as the node stores it: `umx:`, `http://` and `https://` in any case are one scheme, a `umx:` id may be a prefix in any case, and a `?repo=` hint is ignored. Paths and the rest of a URL match only as written. A prefix that names two stored sources is refused with `evidence_ambiguous`. Neither refusal lists the node's pointers.

Treat the `redacted_in_files_only` warning as a to-do. Rewrite the git history that holds the pointer, and redact it in any other graph that cites it, or treat what it pointed at as exposed.

## Don't
- Don't use `--redact` for a stale or wrong pointer; a plain `remove --reason` keeps the pointer in the trail for review (usage 144).
- Don't put any part of the pointer in `--reason`, in any case, and nothing secret-shaped: both are refused.
- Don't add `--at`. Don't use `--redact` with `add` or `list`, or in a batch: it is the verb only.
- Don't expect other graphs to change. A redact rewrites this graph's files only. Repeat it in each graph that cites the source.

## Example
```bash
umx node evidence 01JC8XA4H7 list
umx node evidence 01JC8XA4H7 remove 'notes/creds.md#L3' --redact --reason "the path names a credential"
# {"redacted": {"pointers": 3, "nodes": 2, "edges": 1}}, warning redacted_in_files_only
umx node evidence 01JC8XA4H7 list --removed     # "(redacted)" rows with kind, when, who and the reason

# A plain remove kept a personal URL in the trail; redact it there. A heading word elsewhere does not block.
umx node evidence 01JC8XA4H8 list --removed
umx node evidence 01JC8XA4H8 remove 'https://www.example.com/in/jo-bloggs#section=Experience' --redact --reason "a personal profile"
```

## Why
A plain remove keeps the pointer verbatim so a reviewer can see what was cited. That is wrong when the pointer itself is the leak.

Checking a section heading made every common heading word in the graph block a URL's redact, so a personal URL could not be scrubbed at all. A heading names part of a document; the source is what identifies it. Matching on the source alone, not the selector, is deliberate. Pointers that share a source would otherwise block each other's redact forever. The cost is over-redaction: if only one quote is sensitive, every pointer to that file is redacted too. Security accepted that. A quote-only secret cannot be redacted on its own; re-add the pointers you still need with a selector that does not quote it.

Refusing while the text sits elsewhere stops a redact from reporting success while the secret is still in the files. Counts-only replies keep the pointer out of terminals, logs and agent transcripts.

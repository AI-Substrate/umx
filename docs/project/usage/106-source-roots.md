---
id: 106
title: Cite files outside the graph folder through a source root
route: ingest
status: current
source: ws006 S1 plan 077; graph-addressing-expert R1 (ws006 plan note), architect ws006 shape and S1 port ruling, security ruling ws006 S1, 2026-11-11
reqs: [req-47]
supersedes: none
---
## Do

Cite a file in the graph's own git repository as `root:repo/<path>#<selector>`; it needs no setup.
For a folder elsewhere, ask the human which folder to use (it must be inside the roots), add it once with
`umx root add <name> <folder>`, then cite `root:<name>/<path>#<selector>`. Check roots with `umx root list`
(each row says `ok`, `outside_roots`, `not_a_folder` or `unreadable`).
On `root_unknown`, run the `umx root add` its next names, with the folder filled in.
If `root list` or `root remove` fails with `config_file_invalid` or `config_file_refused`, fix the machine
config file it names, as its next says; umx leaves that file alone until then. `umx root remove <name>`
works even while evidence cites the name: existing pointers keep their spelling, and new citations of that
name get `root_unknown` on this machine until it is added again.

## Don't

Don't cite outside files with `..`, `~` or an absolute path: they are refused (`evidence_outside_roots`).
Don't pick a folder to add on your own, and don't widen `UMX_ROOTS` to make `root add` pass: that is the
human's choice (usage 031). Don't put `source_roots.*` in a repo's `.umx/config.toml`: the repo layer is
ignored for roots. Don't expect umx to read or check the cited file yet; only the root and the path are checked.

## Example

    umx root add acme /Users/me/work/acme
    umx node note 01JC8XA4H7 --append "Agreed in the call" --evidence 'root:acme/transcripts/016.ndjson#seq=1-5' --evidence-kind room
    umx node add --type decision --title "Ship in May" --evidence 'root:repo/docs/plan.md#L12'

## Why

The pointer is stored as written, so it reads the same on every machine; each machine says where its `acme`
is. Old graph-folder-relative pointers keep their meaning. A source root is a folder umx may be asked to read
later, so it must lie inside the roots the human chose, checked on the real path at `root add` and again at
every evidence write.

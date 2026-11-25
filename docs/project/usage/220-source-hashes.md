---
id: 220
title: Cited files are hashed when cited; umx check says which changed
route: ingest
status: current
source: prime packet source-hash (2026-11-25; Jordan: "automatic, first class, when the reference is added")
reqs: [req-77]
supersedes: none
---
## Do
Cite files as before. When a write adds an evidence pointer or a resource locator whose source is a local file (a path under the graph folder, or `root:<name>/<path>`), umx reads the file and stores its `hash` (`sha256:<hex>`), its `length` in bytes and, for a `.ndjson` or `.vtt` transcript, `seq_max` (the highest `seq`, or the cue count), with `hash_state: hashed`. A URL or `umx:` address is `remote` and never fetched. A file over 256 MiB is `too_large` and not read past that.

When a cited file is not there (`missing`), is a folder or refused (`unreadable`), or is a symlink out of the roots (`outside`), the write still goes ahead with that `hash_state`, and warns `source_not_hashed`, whose next is `umx check --graph <alias>`.

Later, run `umx check --graph <alias>` (every check does this since PROTOCOL 102; `--sources` is still accepted and changes nothing). It reads every cited file again and warns `source_changed` (its first stored-`length` bytes hash differently, it is shorter than stored, or a file that was missing is there now) and `source_missing` (it cannot be read now), exit 0, naming the node and the place (`evidence:<i>` or `locator`) in `data.sources.rows`. A file that only grew (longer, its first stored-`length` bytes as stored, like a transcript still being written) is not changed: it is a `source_grew` row, never a warning. Check that what cites the file still holds, then accept the file as it is now with `umx node evidence <node> refresh --at <i>` (or `refresh '<pointer>'`, or `refresh --all`) or `umx resource refresh <node>`. Refreshing a pointer that is still missing records that as accepted, so check leaves it out until it changes. A pointer under a `root:` this machine does not have is left as it was (`source_not_refreshed`). `umx doctor`'s graph check counts them in `doctor_graph_problems`, reading at most 256 MiB of cited files per run (the rest get `doctor_graph_sources_unchecked`). Run the check before you commit and push (usage 222).

## Don't
Don't refresh pointers you did not write without reading them first (`umx node evidence <node> list`): a shared graph can hold a pointer to a file you would not want hashed into a commit, such as `root:repo/.env`. A bare `refresh` is refused for that reason; after `--all`, read `data.hashed`. Don't hand-edit `hash` in nodes.jsonl: refresh it. Don't expect a read to re-hash: only writes, `check` and refresh read files. Don't expect the same pointer added again to refresh its hash: it keeps the one stored. Don't expect edge evidence to be checked: it is hashed on add, and `check` reads nodes' evidence and locators only.

## Example
```bash
umx node add --type decision --title "Ship in May" --evidence 'calls/3-nov.ndjson#seq=120-140' --evidence-kind room
umx check --graph launch
umx node evidence 01JC8XA4H7 refresh --at 1
umx resource refresh 01JD7K2M3N
```

## Why
A pointer is only as good as the file it points at. A transcript that is re-cut or a document that is edited silently moves what a citation means. The hash, taken when the reference was added, lets umx say which citations to re-check, and refresh records that a person looked.

---
id: 011
title: Every claim carries evidence
route: ingest
status: planned
source: D10 (2026-11-09); req-18
reqs: [req-18, req-58]
supersedes: none
---
## Do
Cite where a node's content came from with `--evidence <pointer>` and `--evidence-kind room|chat|screen|document|analyst`: `transcripts/016.ndjson#seq=3206-3240`, `docs/brief.md#L10-L24`, or `#quote=<text>`. Append later evidence with `umx node note <id> --append "..." --evidence <pointer>`.

## Don't
Don't cite captions as verbatim quotes (they are lossy); quote chat or documents. Don't guess speaker names; cite sequence numbers. Mark your own inferences as `analyst`. Once a resource node exists for the source (note 012), cite it by its address with a selector (`umx:<alias>/<transcript id>#seq=3206-3240`) rather than the file path, so every citation points at the one node.

## Example
```bash
umx node note <id> --append "Finance pushed back" --evidence transcripts/016.ndjson#seq=4100-4120 --evidence-kind room
```

## Why
Written beats spoken; chat beats captions; analyst inferences must stay distinguishable from the room's words.

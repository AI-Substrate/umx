---
id: 012
title: Outside things are resource nodes, one per locator
route: resources
status: planned
source: D34 (2026-11-10); workshop 010; req-77
reqs: [req-77]
supersedes: none
---
## Do
Make a transcript, file, document or URL a node with `--locator`. Adding the same locator again returns the existing node, so it is safe to call before linking.

## Don't
Don't create a second node for a file that already has one. Don't paste file contents into notes; point at them.

## Example
```bash
umx node add --type transcript --locator transcripts/016-transcript.ndjson
```

## Why
One node per outside thing keeps every link pointing at the same place.

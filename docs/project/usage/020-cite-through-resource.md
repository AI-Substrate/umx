---
id: 020
title: Cite passages through the resource node, with the selector its format supports
route: ingest
status: planned
source: workshop 010 sections 2, 4 and 12 (graph-addressing-expert, 2026-11-10); D35
reqs: [req-18, req-77, req-78, req-81]
supersedes: none
---
## Do
This refines note 011 once resource nodes exist (req-77). Until then, 011's path form (`transcripts/016.ndjson#seq=...`) is the one that works, and stays correct for files with no resource node.

Once a file or transcript has a resource node (note 012), cite it by address, in evidence and in edges alike: `umx:acme/<T>#seq=3206-3240`, not `transcripts/016.ndjson#seq=3206-3240`. Pick the selector the format supports: `seq=` for transcripts, `section=` or `L` for markdown and text, `section=` or `quote=` for Word. Add `&quote=<text>` when the source may still be edited, because the quote finds the passage again if the numbers drift. When you need the words, run `umx resolve <address>`.

## Don't
Don't paste passage text into a node to keep it; the address is the copy. Don't use `L` line numbers on a transcript (sequence numbers are the entries' own ids). Don't use absolute paths; outside the repo use `root:<name>/<path>`. Don't put a selector on a node that has no locator (`not_a_resource`).

## Example
```bash
umx node add --type decision --title "Ship in March" \
  --evidence 'umx:acme/<T>#seq=3206-3240&quote=we%20can%20ship%20in%20March' --evidence-kind room
umx resolve 'umx:acme/<T>#seq=3206-3240'
```

## Why
A citation by node id survives the file moving and lets `--evidence-from <T>` and `umx backlinks` find it. One selector grammar serves addresses, evidence and edges, so what you cite you can also link to and later promote.

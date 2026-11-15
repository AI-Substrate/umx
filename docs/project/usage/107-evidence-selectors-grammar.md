---
id: 107
title: Evidence selectors: section, page, several at once, and encoded text
route: ingest
status: current
source: workshop 010 section 2; graph-addressing-expert ws006 plan (R2) and S2 reply 2026-11-11; plan 078
reqs: [req-18, req-78]
supersedes: none
---
## Do
Pick the selector that names the passage most exactly: `seq=` for transcript entries, `L10-L24` or `section=Scope/Risks` (a heading path, top level first) for documents, `page=3-4` for PDFs, `quote=<text>` with `&prefix=`/`&suffix=` anywhere. Join several with `&` when a quote should back up numbers that may drift: `--evidence 'transcripts/016.ndjson#seq=4100-4120&quote=we ship in March'`. Single-quote the pointer in the shell. When citing a node in another repo's graph, put its `?repo=<git url>` hint last, after the selectors (`umx:launch/<id>#L3-L4?repo=https://...`); umx keeps it on the stored source. Raw spaces are fine; umx decodes `%XX`, so a `/` inside one heading is `%2F` and a literal `%20` is `%2520`.

## Don't
Don't repeat a kind (`seq=1-2&seq=5-6` is refused) or give `prefix=`/`suffix=` without `quote=`; use two `--evidence` flags for two passages. Don't put a field name after `#` in evidence (`a.md#problem`); evidence needs a selector. A refused pointer is `evidence_pointer_invalid` with the reason, and nothing is written.

## Example
```bash
umx node note <id> --append "Scope risk raised" --evidence 'docs/brief.md#section=Scope/Risks' --evidence 'calls/3-nov.ndjson#seq=120-140&quote=we ship'
```

## Why
One grammar (workshop 010) serves evidence, addresses and, later, edge ends, so a pointer you write now resolves the same way once umx reads sources. A quote alongside numbers still finds the passage if the numbers drift.

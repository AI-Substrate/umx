---
id: 021
title: A live transcript grows; that is not a change
route: resources
status: planned
source: workshop 010 section 5 (graph-addressing-expert, 2026-11-10)
reqs: [req-82, req-61]
supersedes: none
---
## Do
Add a live transcript once, as a resource node, and keep citing it while it grows. Treat `grown` from `umx resource check` as normal. Cite new entries by `seq=` straight away; they resolve with `beyond_recorded` until the next pass records them. On `source_changed`, resolve the passages you rely on again and check their words, because an earlier entry was edited.

## Don't
Don't add the transcript again as a new resource because it has grown (adding it returns the same node anyway). Don't treat `grown` as an error. Don't drop links that report `selector_not_found`; `umx check` lists them for a person to decide.

## Example
```bash
umx resource check <T>             # grown
umx resolve 'umx:acme/<T>#seq=3600-3612'   # beyond_recorded, still returns the entries
```

## Why
Sequence numbers name entries, not bytes, so citations stay exact while the file grows or an earlier caption is corrected.

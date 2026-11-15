---
id: 110
title: Set UMX_AUTHOR once so every write says who made it
route: build
status: current
source: ws002 s14 (plan 080), the project manager 2026-11-11; documents existing behaviour (workshop 007 rule 8)
reqs: [req-12, req-93]
supersedes: none
---
## Do
- Set `UMX_AUTHOR` to your seat or role name once, before your first write. umx records it as `created_by` on new nodes and as `by` on each status move and `umx node history` row.
- Use `--by <name>` only for a single write made on someone else's behalf.

## Don't
- Don't leave it unset when several agents write to one graph: every write then shows the OS user, so history can't tell you apart.
- Don't pass `--by` on every command as a substitute for `UMX_AUTHOR`; it is easy to forget once.

## Example
```bash
export UMX_AUTHOR=pm-001
umx node status 01JC8XA4H7 agreed --reason "signed off on the 3 Nov call"
umx node history 01JC8XA4H7
```

## Why
`umx node history` is how a teammate finds who moved an argument and why (req-12). Without `UMX_AUTHOR` or `--by`, umx falls back to the OS user, which is the same for every agent on one machine. Workstream 007 (req-93) now records the session and agent of every write by itself (`umx docs get node`, Who wrote it); `created_by` and `by` still come from `UMX_AUTHOR` or `--by`, so keep setting it.

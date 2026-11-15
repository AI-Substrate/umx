---
id: 003
title: Graphs live under your roots
route: start
status: superseded
source: workstream 001 s05a; project lead and architect ruling 2026-11-09
reqs: [req-47]
supersedes: none
superseded_by: 031
---
## Do
Put graph folders under `$HOME` or `$TMPDIR` (the default roots). On macOS `/tmp` is neither.

## Don't
Don't ask to widen `UMX_ROOTS` unless the human asked; the tool deliberately never suggests widening its own file access.

## Example
```bash
umx graph new ~/graphs/acme --alias acme
```

## Why
Locators and graph folders are confined to roots for safety (req-47).

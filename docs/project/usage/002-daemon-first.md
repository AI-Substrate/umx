---
id: 002
title: Start the daemon yourself; nothing auto-starts
route: start
status: current
source: D13, D13a (2026-11-09); req-38
reqs: [req-38]
supersedes: none
---
## Do
Run `umx daemon start` in another window and leave it running. `umx version`, `umx agents-start-here`, `umx docs list|get` and `umx daemon status` work without it; everything else needs it.

## Don't
Don't expect auto-start. Don't read or edit graph files directly to work around a stopped daemon.

## Example
```bash
umx daemon status
umx daemon start   # in another window
```

## Why
One writer keeps files consistent and merge-friendly (D13).

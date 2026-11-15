---
id: 090
title: daemon_busy means the daemon is serving too many requests at once; wait and run the command again, except after umx daemon restart, where you check first
route: doctor
status: current
source: F56, plan 057, 2026-11-11; architect yes; F67 (review R129-F1, R137-B2)
reqs: [req-38]
supersedes: none
---
## Do
- On `daemon_busy` (exit 1) from any command but `umx daemon restart`, wait a moment and run the same command again: the daemon is already serving as many connections as it takes at once (64), and the command did not run, writes included. For `umx daemon restart`, see below before running it again.
- Use `umx daemon status`, `umx daemon stop` and `umx daemon restart` as usual: the daemon keeps 8 more connections for them alone, so they still answer when it is busy.
- Read `daemon_busy` from `umx daemon restart` as a running daemon, never a stopped or older one, so don't run `umx daemon start` for it. Whether the restart happened depends on when the daemon was busy: before it took the restart, nothing was restarted; while restart was waiting for the new daemon to answer, the restart may already have happened. Run `umx daemon status` and compare `started_ms` (or `uptime_s`) with what you expect before restarting again.
- If `daemon_busy` keeps coming back, something is holding connections open without finishing its requests: run `umx doctor`, and `umx daemon restart` to clear them.

## Don't
- Don't retry in a tight loop: wait first, since busy means other commands are still running.
- Don't read it as a stopped daemon: `daemon_not_running` (exit 3) is that one, and `umx daemon start` does not help here.

## Example
```bash
umx graph list        # daemon_busy: The daemon is serving too many requests at once; nothing ran.
sleep 2; umx graph list
umx daemon status     # still answers while the daemon is busy
```

## Why
Each connection holds a daemon thread until its request is read and answered, up to 120 s for a client that stalls (usage 089). A cap keeps a stuck or runaway client from using up the daemon, and the reserved slots keep the commands that check, stop and restart it working.

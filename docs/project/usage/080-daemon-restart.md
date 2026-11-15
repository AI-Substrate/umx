---
id: 080
title: umx daemon restart restarts a running daemon, through its service or in place, and never starts one from nothing
route: doctor
status: current
source: F46 (plan 045), 2026-11-10; review round 1 B1-B5; F52 (review follow-up F1); F55
reqs: [req-85, req-86]
supersedes: none
---
## Do
- Run `umx daemon restart` when a reply says `config_restart_needed`, when `umx daemon status` shows `writer: failed`, or when a `umx doctor` finding's `next` is `umx daemon restart`. It waits until every write sent before it has been applied, starts the daemon again, and replies once the new daemon answers.
- Read `how` in the reply: `service` means the installed service was the daemon for this `UMX_HOME`, so it stopped and launchd or systemd started it again; `in_place` means the daemon replaced itself with the same pid, flags and environment, running the umx binary from the path it was started from. A service for another `UMX_HOME` is never touched.
- On `service_failed`, run `umx daemon status` to see where it stopped, and look for `pid`, not only `running`. A reply with a `pid` means a daemon still answers: the service could not be inspected and nothing changed (`running: false` with `writer: failed` is a live daemon whose writer had already failed, not one the restart stopped). A reply with no `pid` (only `running: false` and the socket) means the daemon had applied its queued writes and stopped, and then launchd or systemd failed or timed out starting it again: run `umx daemon install` (it starts the service again) or `umx daemon start`, then `umx doctor` if it still fails.
- Run a command again if it failed with `daemon_not_running` while the restart was under way.
- On `daemon_restart_refused` (a daemon older than this command), run `umx daemon stop`, then `umx daemon start`.

## Don't
- Don't use it to start a daemon: with none running it fails with `daemon_not_running` and starts nothing, even when the service is installed but stopped.
- Don't retry it in a loop when it fails with `daemon_not_running` after a wait: it gives the daemon 10 s to answer, 60 s to apply queued writes and 10 s to come back. Run `umx doctor`.
- Don't expect it to switch to another umx binary: an in-place restart runs the binary the daemon was started from. After installing umx somewhere else, stop the daemon and start it with the new one (or `umx daemon install` again for a service).
- Don't worry about an embed round in progress: the restart drops it, and it runs again when each graph loads.

## Example
```bash
umx config set daemon.idle_unload 1h  # warns config_restart_needed
umx daemon restart                    # {"how":"in_place","pid":4242,...}
```

## Why
Stopping and starting by hand meant finding the window that ran the daemon, losing its flags, or racing a service that restarts itself. One command that drains writes and comes back with the same settings makes restart a safe `next` for doctor and config.

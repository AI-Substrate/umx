---
id: 025
title: protocol_mismatch, writer_stopped and graph_format_newer: restart or upgrade, don't retry
route: start
status: current
source: review of 882fd36 (A6, A7, A2), blueprint 001 E-4
reqs: [req-38]
supersedes: none
---
## Do
- `protocol_mismatch`: the CLI and the daemon are different builds that speak different protocols, and nothing ran. Stop the daemon and start it again with the CLI you are using: `umx daemon stop`, then `umx daemon start` in another window.
- `internal` with `writer_stopped`: a write failed inside the daemon, so it now refuses every command (reads too) except `umx daemon status` and `umx daemon stop`. `umx daemon status` shows `writer: failed`. Stop and start the daemon; the files hold every acknowledged write.
- `graph_format_newer`: a newer umx wrote this graph. Nothing was loaded. Check `umx version`, install a newer umx, then restart the daemon.

## Don't
Don't retry the same command in a loop: these do not clear by themselves. Don't edit graph.json's `umx` number to get a newer graph to load.

## Example
```bash
umx daemon status    # writer: failed, or a different version
umx daemon stop
umx daemon start     # in another window
```

## Why
A daemon that cannot trust its memory, or that would read a request or a file differently from how it was written, must not answer as if all were well.

---
id: 087
title: The CLI waits for a reply up to 600 s for a write, 120 s for a read, 60 s for status; internal reply_timed_out means it gave up
route: doctor
status: current
source: F50, plan 055, 2026-11-10
reqs: [req-38]
supersedes: none
---
## Do
- Expect a command to wait while the daemon is busy: a write may queue behind a graph load, a batch or a similarity rebuild, so the CLI waits up to 600 s for its reply (also for `umx daemon stop` and `umx daemon restart`), up to 120 s for a read, and 60 s for `umx daemon status` (a daemon that is still starting answers only once it is ready).
- When a command fails with `internal` and the message names `reply_timed_out`, the CLI stopped waiting: the daemon did not answer in time, or its reply stalled for 10 s or took more than 60 s once it had started. For a write, check whether it ran (`umx node get <id>`, `umx node list`, `umx graph list`) before running it again.
- Run `umx doctor` if it repeats, and report the command, the full JSON reply and `umx version`.

## Don't
- Don't rerun a timed-out write blindly: it may have run, and a second `node add` makes a second node.
- Don't read `reply_unreadable` as a timeout: the daemon closed the stream before the reply was whole.

## Example
```bash
umx node add --type problem --title "x"   # internal: Internal error reply_timed_out.
umx node list --type problem              # did it land?
umx doctor                                # if it keeps happening
```

## Why
The CLI used to wait for a reply with no limit at all, so a daemon that never answered, or sent its reply a byte at a time, hung every command forever (a test teardown hung 52 minutes this way). The limits follow what the request does, so a slow but working daemon is still waited for.

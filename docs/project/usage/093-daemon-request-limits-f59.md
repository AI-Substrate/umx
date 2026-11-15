---
id: 093
title: The daemon waits 60 s for a request's first byte and 10 s for each later pause, 120 s in all; request_timed_out means the request did not complete in time
route: doctor
status: current
source: F59, plan 060, 2026-11-11; architect note 11-note-f-59; F57 (review R117-F1)
reqs: [req-38]
supersedes: 089
---
## Do
- Expect a busy machine to be tolerated. After the CLI connects, the daemon waits up to 60 s for the first byte of the request, up to 10 s for each later pause, and at most 120 s for the whole request. A normal umx command sends its whole request at once, so none of these limits is near.
- When a command fails with `internal` and the message names `request_timed_out`, the request did not complete: the daemon stopped waiting before the request's end arrived, and nothing ran. Usually the machine was overloaded or the client stalled. Run the command again. If it keeps happening, run `umx doctor` and report the command, the full JSON reply and `umx version`.
- Read `internal` with `request_unreadable` as different: the request arrived but could not be read or parsed (often a CLI and daemon from different builds). `umx doctor` says which.

## Don't
- Don't retry `request_timed_out` in a tight loop on a busy machine; wait a moment first.
- Don't write a client that opens the socket and sends nothing, or sends part of a request and stops: the daemon closes it after 60 s, or 10 s into a pause.

## Example
```bash
umx node add --type problem --title "x"   # internal: Internal error request_timed_out.
umx node add --type problem --title "x"   # run again once the machine is less busy
umx doctor                                # if it repeats
```

## Why
Note 089 allowed 60 s for a pause because a `request_timed_out` on loaded macOS runners looked like a client stalling before it finished its request (F54). The real cause was in the daemon (F59): macOS can miss the wakeup for a request's end that arrives while a read waits, so the read slept until its limit with the whole request already there. Every read now waits at most 250 ms and looks again, so a complete request is always read, and the pause limit is back to 10 s.

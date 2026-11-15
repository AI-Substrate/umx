---
id: 077
title: The daemon waits up to 60 s for a request; internal request_timed_out means the request never arrived
route: doctor
status: superseded
superseded_by: 089
source: F44 (plan 038), 2026-11-10; review R93-1
reqs: [req-38]
supersedes: none
---
## Do
- Expect a slow machine to be tolerated. After the CLI connects, the daemon waits up to 60 s for the first byte of the request, cuts off a request that stops for 10 s once it has started, and gives any request at most 120 s in all. A normal umx command sends its whole request at once, so none of these limits is near.
- When a command fails with `internal` and the message names `request_timed_out`, the daemon gave up waiting for the request: the machine was overloaded, or the client stalled. Run the command again. If it keeps happening, run `umx doctor` and report the command, the full JSON reply and `umx version`.
- Read `internal` with `request_unreadable` as different: the request arrived but could not be read or parsed (often a CLI and daemon from different builds). `umx doctor` says which.

## Don't
- Don't retry `request_timed_out` in a tight loop on a busy machine; wait a moment first, since load is the usual cause.
- Don't write a client that opens the socket and sends nothing: the daemon closes it after 60 s.

## Example
```bash
umx node add --type problem --title "x"   # internal: Internal error request_timed_out.
umx node add --type problem --title "x"   # run again once the machine is less busy
umx doctor                                # if it repeats
```

## Why
One 10 s limit per read cut off live clients on a loaded CI runner. The daemon is local and talks only to umx processes that already connected, so a late first byte means a busy machine, while a stall in the middle of a request, or a request that never ends, is still cut off so no connection holds a daemon thread for long.

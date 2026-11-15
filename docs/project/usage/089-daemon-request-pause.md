---
id: 089
title: The daemon waits 60 s for a request's first byte and 60 s for each later pause, 120 s in all; request_timed_out means it never arrived
route: doctor
status: superseded
superseded_by: 093
source: F54, plan 056, 2026-11-11; architect yes
reqs: [req-38]
supersedes: 077
---
## Do
- Expect a busy machine to be tolerated. After the CLI connects, the daemon waits up to 60 s for the first byte of the request, up to 60 s for each later pause, and at most 120 s for the whole request. A normal umx command sends its whole request at once, so none of these limits is near.
- When a command fails with `internal` and the message names `request_timed_out`, the daemon gave up waiting for the request: the machine was overloaded, or the client stalled. Run the command again. If it keeps happening, run `umx doctor` and report the command, the full JSON reply and `umx version`.
- Read `internal` with `request_unreadable` as different: the request arrived but could not be read or parsed (often a CLI and daemon from different builds). `umx doctor` says which.

## Don't
- Don't retry `request_timed_out` in a tight loop on a busy machine; wait a moment first, since load is the usual cause.
- Don't write a client that opens the socket and sends nothing, or sends part of a request and stops: the daemon closes it after 60 s.

## Example
```bash
umx node add --type problem --title "x"   # internal: Internal error request_timed_out.
umx node add --type problem --title "x"   # run again once the machine is less busy
umx doctor                                # if it repeats
```

## Why
Note 077 allowed 10 s between bytes once a request had started. On loaded macOS CI runners a live CLI stalled longer than that between sending its request and closing its side, and failed with `request_timed_out` (F54). The pause is the same busy machine as a slow first byte, so it now gets the same 60 s; the 120 s total still frees the daemon from a client that never finishes.

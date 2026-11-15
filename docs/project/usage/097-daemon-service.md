---
id: 097
title: Ask `umx daemon service` whether the service is installed; status drops it when the daemon fails
route: doctor
status: current
source: F58, architect ruling 2026-11-11 (option c, no wire change), the project manager; PR for F58
reqs: [req-86]
supersedes: none
---
## Do
- To learn whether the daemon runs as a service (installed, loaded, its pid, the umx binary it runs), run
  `umx daemon service`. It asks launchd or systemd only and never the daemon, so it answers the same whether the
  daemon is up, down, slow or refusing. It prints `{ "service": { ... } }`, the same object `umx daemon status`
  shows under `service`.
- In scripts, test `"installed":true` in `umx daemon service --json`, not in `umx daemon status`.
- If it fails with `service_failed` (exit 4), launchd or systemd could not be read: run `umx doctor`.

## Don't
- Don't read a missing `service` in `umx daemon status` as "no service". Status adds `service` only when the
  daemon answers or none is running; when the daemon is slow or refuses (`internal` with `reply_timed_out`,
  `protocol_mismatch`), status is an error, and an error envelope's `data` is always null.
- Don't use `umx daemon service` to learn whether the daemon itself is running: that is `umx daemon status`
  (`running`). A loaded service with a pid can still have a daemon that does not answer.

## Example
```bash
umx daemon service --json | grep -q '"installed":true' && echo "runs as a service"
umx daemon status
```

## Why
A slow daemon made `just dev` read an installed service as none and start a second daemon by hand (F58, seen in
macOS CI). Error envelopes keep `data: null` for every command (workshop 007), so the service view needed its own
command rather than an exception on status's error path.

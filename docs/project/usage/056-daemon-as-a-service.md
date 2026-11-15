---
id: 056
title: The daemon as a service: install only when asked; restart it with umx daemon install, not by hand
route: start
status: current
source: req-86 (decision D39, 2026-11-10); architect ruling on plan 024, 2026-11-10; PR for ws002 s10
reqs: [req-86]
supersedes: none
---
## Do
- Check `umx daemon status`: `service.installed`, `service.loaded` and `service.pid` say whether the human runs umx as a per-user service (a launchd agent on macOS, a systemd user unit on Linux).
- When the service is installed but its daemon is down (after `umx daemon stop`, or `doctor_service_down`), run `umx daemon install`. It rewrites the service file and starts it again with this umx.
- After upgrading umx, `just dev` (in the umx repo) or `umx daemon install` makes the service run the new build.
- Put graph roots in config: the service does not see your shell's `UMX_ROOTS`.

## Don't
- Don't install or uninstall the service unless the human asked: it adds or removes a login item.
- Don't start a second daemon by hand while the service is installed. `umx daemon start` refuses while one runs, and a hand-started daemon makes the service stand aside (`doctor_service_beside`) until you stop it.
- Don't expect `umx daemon stop` to be undone by the service: a clean stop stays stopped until the next login or `umx daemon install`. Only a crash brings it back by itself.
- Don't use sudo: everything is per user.
- Don't set `UMX_SERVICE_TOOL` outside tests. It replaces `/bin/launchctl` or `systemctl`; `service.tool` in `umx daemon status` shows the one in use, and with it set away from `~/.umx` the service gets a sandbox label so the real one is never touched.

## Example
```bash
umx daemon install     # data.service.installed: true, loaded: true
umx daemon status      # service.pid is the running daemon
umx daemon stop        # stopped; the service does not restart it
umx daemon install     # running again
umx daemon uninstall   # gone; UMX_HOME and daemon.log stay
```

## Why
A daemon that starts at login and comes back after a crash means agents find umx running. Restarting only through the service manager keeps one daemon per machine, and `--service` standing aside for a running daemon stops launchd or systemd from retrying in a loop.

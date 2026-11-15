---
id: 055
title: umx doctor: run it when something is wrong; exit 5 is degraded, 6 is broken; --fix only removes a stale socket and tightens modes
route: doctor
status: current
source: req-85 (decision D39, 2026-11-10); architect ruling on plan 024, 2026-11-10; PR for ws002 s09
reqs: [req-85]
supersedes: none
---
## Do
- Run `umx doctor` when a command fails in a way its `next` does not clear, after installing or upgrading umx, or before a long session. It works with the daemon down.
- Read `data.verdict` or the exit code: 0 ok (information findings included), 5 degraded (umx works but something needs attention), 6 broken (something umx needs is unsafe or unusable). Exit 2 still means the command line was wrong.
- Run each finding's `next`. Every finding is a warning whose code starts `doctor_`; `data.rows` has one row per check with `ok`, `info`, `degraded` or `broken`.
- Run `umx doctor --fix` for `doctor_socket_stale` and `doctor_mode_open`. It says each repair on stderr before it runs, lists them in `data.fixed`, then checks again.

## Don't
- Don't expect `--fix` to start or stop the daemon, edit config, widen the roots, touch graph files, or change a symlink or a path you do not own. It only removes a socket nothing answers on and clears group and other bits.
- Don't treat `doctor_embedder_unready` as a umx bug: for Azure it means `az login` (or `azd auth login`) has no token for this user; log in, then run `umx doctor` again.
- Don't treat `doctor_embedder_none` as a fault: no embedder only turns similarity search off. With the daemon down, doctor checks no graph (umx keeps no list of graphs yet); start the daemon and load them to have them checked.
- Don't stop at fixing a config file when doctor says `doctor_daemon_config_closed`: the running daemon keeps the config it started with, so restart it (`umx daemon stop`, then start it again).
- Don't loop on `umx doctor`. If a `next` does not clear its finding, report the full JSON reply.

## Example
```bash
umx doctor            # exit 5: doctor_daemon_down, next: umx daemon start
umx daemon start      # in another window
umx doctor            # exit 6: doctor_mode_open on ~/.umx (mode 755)
umx doctor --fix      # stderr: umx doctor --fix: chmod 700 /Users/me/.umx
umx doctor            # exit 0, verdict ok
```

## Why
An agent that hits a setup problem should not have to guess which of a dozen things is wrong. One offline command checks them in a fixed order and names the command that fixes each. `--fix` is limited to repairs that cannot lose data or widen access, so it is always safe to run.

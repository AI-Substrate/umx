---
id: 033
title: The daemon's run folder must be yours and private
route: start
status: current
source: daemon run folder owner check
reqs: [req-45]
supersedes: none
---
## Do
If `umx daemon start` refuses with `socket_unavailable`, read the cause and fix the folder it names. If the run folder belongs to another user, remove it or set `UMX_HOME` to a folder you own. If it is open to other users, `chmod 700` it. Then start again.

## Don't
Don't chmod or chown a folder another user owns to get past the refusal, and don't share one `UMX_HOME` between users. Don't retry `daemon start` without changing anything.

## Example
```bash
umx daemon start                     # socket_unavailable: the run directory belongs to another user
UMX_HOME="$HOME/.umx" umx daemon start
```

## Why
Only the owner may reach the daemon socket. A run folder made by someone else could let that user watch or replace it.

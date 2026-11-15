---
id: 031
title: Roots come from config; an invalid config closes every graph folder
route: start
status: current
source: workstream 002 s07 (req-72), architect ruling 2026-11-10 (fail closed without refusing to start)
reqs: [req-47, req-72]
supersedes: 003
---
## Do
Put graph folders under your roots: `$HOME` and `$TMPDIR` by default (on macOS `/tmp` is neither).
`umx config show` prints the roots and where they came from (`roots` in the machine config, or `UMX_ROOTS`).
If a graph command fails with `config_invalid`, run `umx config check` and fix what it names. For `roots`, ask
the human which folders to use (umx never suggests them), set them with `umx config set roots <folders>`, then
restart the daemon (`umx daemon stop`, then `umx daemon start`).

## Don't
Don't widen the roots (`umx config set roots ...` or `UMX_ROOTS`) unless the human asked; the tool deliberately
never suggests widening its own file access. Don't treat `config_invalid` as a path problem: no folder works
until the config is fixed and the daemon restarts.

## Example
```bash
umx graph new ~/graphs/acme --alias acme
umx config check
```

## Why
Graph folders and locators are confined to roots for safety (req-47). An invalid roots value, or a machine
config file that is refused or does not parse, leaves no roots rather than the wider default, and the daemon
still starts so `umx config check` can say what to fix.

---
id: 101
title: umx --version and umx -V are umx version
route: start
status: current
source: F70 (field report from a tester via the project lead, 2026-11-11), plan 070
reqs: [req-38, req-63, req-70]
supersedes: none
---
## Do

Use `umx version`, `umx --version` or `umx -V` to check the binary. All three are the same command: the same
envelope (`command` is `version`), the same `--json`/`--human` rules and the same exit code. Like
`umx version`, they work with the daemon down and then report `daemon: null`.

## Don't

Don't expect other leading flags to work: `umx --verbose node list` and other unknown flags before the command
still refuse with `usage_invalid` (exit 2). The alias only counts as the first word, so `umx node add -V` is
not a version check. `umx --version extra` refuses like `umx version extra`.

## Example

    umx --version
    umx -V --human
    umx --version --help     # help for version, like umx version --help

## Why

Agents and people try `--version` first, as with most tools. Before F70 it gave `usage_invalid`
("unknown flag before the command"), which looks like a broken install.

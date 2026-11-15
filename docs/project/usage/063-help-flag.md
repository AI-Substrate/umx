---
id: 063
title: Help: `umx --help` for the overview, `umx <noun> --help` for a noun's docs, offline
route: start
status: current
source: project manager packet ws002 s12, 2026-11-10 (the project lead's smoke test: --help was refused); PR for ws002 s12
reqs: []
supersedes: none
---
## Do
- Unsure what a noun takes? Run `umx <noun> --help` (or `-h`). It prints that noun's docs topic, the same text as `umx docs get <noun>`, with the daemon up or down. After a verb it shows the noun's topic too: `umx node add --help`.
- Run `umx --help` (or `umx help`) for the overview: what umx is, every noun with one line, and the commands that work without the daemon.
- Help follows the output rules: one JSON envelope (the text is in `data.markdown`) when piped or with `UMX_OUTPUT=json`, text on a terminal.

## Don't
- Don't pass `--help` or `-h` as a value (a title, a note): any `--help` or `-h` on the line asks for help and nothing runs.
- Don't retry an unknown word: `help_unknown` (exit 2) lists the nouns as `candidates`; its `next` is `umx --help`.

## Example
```bash
umx --help
umx node --help
umx daemon start --help   # help, the daemon does not start
```

## Why
Every agent tries `--help` first. Before this, `umx --help` was refused and `umx node --help` said "unknown command".

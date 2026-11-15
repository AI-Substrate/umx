---
id: 053
title: Change a graph's settings with graph set; run graph new's next lines
route: build
status: current
source: project lead ruling 2026-11-10 (generic graph set)
reqs: [req-71]
supersedes: none
---
## Do
After `umx graph new`, read `data.next` (JSON) or the `next:` lines (`--human`): commands that add a type, a
relation and a status and set the title, each runnable as printed. Run or adapt them. Change a graph setting
with `umx graph set <alias> --<setting> <value>`; the flag is the setting's snake_case name. Title is the only
setting today.

## Don't
Don't edit graph.json by hand to change a setting; every change goes through the CLI. Don't guess setting
names: an unknown one is refused with `setting_unknown`, whose message lists the settings, and nothing
changes.

## Example
```bash
umx graph new ~/graphs/acme --alias acme
umx ontology add type constraint --graph acme
umx graph set acme --title "Acme onboarding"
```

## Why
Templates were ruled out (D30); the path to shape each graph must be obvious from its first command. The
settings live in one table in umx-core, so later settings (similar_k, embedder, secured) arrive as one row
each with the same verb.

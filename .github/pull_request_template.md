Delivers: <full ddocs addresses this PR delivers, or none>
Fixes: <F-items, such as F119, F117; delete this line if none (the delivery check refuses "Fixes: none")>
Docs: <each one touched, or why none: the docs topic (crates/umx-cli/docs/ or docs.rs), the usage note (docs/project/usage/NNN), agents-start-here, the skill route (skills/umx/references/routes/)>
Hints: <the hint codes this PR adds or changes, or why none (D69: a read that hides, skips, truncates or defaults something says so with a hint naming the command that shows it)>

<!-- Docs are part of done. A new or changed command, flag, default, warning or refusal needs: its docs topic
     (`just check` fails while a parsed flag is missing from the topic its `umx <cmd> --help` prints), a usage
     note in the docs/project/usage/README.md format, a link from agents-start-here for a new topic, and the
     note cited as `usage NNN` in its skill route. -->

## What and why

## Gate

At the final head, with main merged:

- `timeout 3600 just check`: exit
- `timeout 1800 scripts/cargo-slot test -p umx --locked --test writers -- --ignored`: exit

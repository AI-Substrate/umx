---
id: 072
title: The .gitignore check matches git on [[:class:]] brackets and on core.ignorecase either way
route: doctor
status: current
source: reviewer follow-up FU67-1; workstream 003 follow-ups 2 (plan 036, 2026-11-10)
reqs: [req-04]
supersedes: none
---
## Do
Read `gitignore_line_missing` (note 058) as "git, on some machine, would show `.umx-tmp-` files". umx now reads
every `.gitignore` line the way git does, including `[[:digit:]]` style classes, ranges, escapes, trailing
spaces and CR line ends, and it checks both values of git's `core.ignorecase` (false on Linux, true by default on
macOS). So a line such as `!.UMX-TMP-*` or `!.umx-tmp-[[:digit:]]*` counts as un-hiding. The fix is still to add
`.umx-tmp-*` as the last line.

## Don't
Don't keep a `!` line that matches `.umx-tmp-` names in a different case, or through a class, expecting it to be
harmless: on a macOS checkout git applies it.

## Example
```bash
printf '.umx-tmp-*\n!.UMX-TMP-*\n' > /Users/me/work/launch-graph/.gitignore
umx check --graph launch     # gitignore_line_missing: a case-insensitive git shows the temp files
echo '.umx-tmp-*' >> /Users/me/work/launch-graph/.gitignore
umx check --graph launch     # no gitignore row
```

## Why
The check is tested against real `git check-ignore` over hundreds of generated `.gitignore` files under both
`core.ignorecase` values. Before this, a `[[:digit:]]` negation or a different-case negation could un-hide temp
files while umx said they were hidden.

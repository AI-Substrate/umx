---
id: 058
title: umx check warns when .gitignore stops hiding .umx-tmp- files
route: doctor
status: current
source: workstream 003 s03 follow-ups (plan 028, 2026-11-10); follow-ups from security and graph-storage-expert
reqs: [req-04]
supersedes: none
---
## Do
When `umx check` reports `gitignore_line_missing`, add the line `.umx-tmp-*` at the end of the graph folder's
`.gitignore`, with nothing before it on the line. At the end, it also wins over an earlier `!` line. Run
`umx check` again to confirm.

## Don't
Don't add a `!` line after it that matches `.umx-tmp-` names (`!.umx-tmp-*`, `!*`, `!.*`): in git the last
matching line wins, and the temp files show up again.

Don't commit `.umx-tmp-` files that git already shows; add the line, then check `git status`.

## Example
```bash
umx check --graph launch     # warning gitignore_line_missing, row kind gitignore_line_missing
echo '.umx-tmp-*' >> /Users/me/work/launch-graph/.gitignore
umx check --graph launch     # no gitignore row
```

## Why
umx writes temp files and a marker, `.umx-tmp-set.json`, in the graph folder while it writes, and leaves them
after a crash until the next command finishes the write (note 052). `umx graph new` adds the line; this catches
a `.gitignore` edited or merged since.

---
id: 088
title: A config, secrets or state file you cannot open is refused with its chmod repair, not write_failed
route: doctor
status: current
source: F51, review R108-F1, project manager 2026-11-10; plan 054
reqs: [req-72]
supersedes: none
---
## Do
- If `config_file_refused` (or a refused secrets or state file in `umx doctor`) says group or others can write
  or read it and to run chmod 600, that is a permission problem with umx's own file. Run `chmod 600` on the
  file it names, then check with `umx doctor`. This now happens even when the owner cannot open the file at all
  (for example mode 0022 on a config, or 0044 on `secrets.env` or `state/graph-policies.json`); before this fix
  such a config showed up as `write_failed` with exit 4.
- After repairing the machine config (`$UMX_HOME/config.toml`), run `umx daemon restart`. A refused machine
  config closes every graph folder at start, and the folders open again only when the daemon restarts with a
  trusted file. A repaired repo `.umx/config.toml`, secrets file or state file is read again on next use.

## Don't
- Don't treat this as a roots problem. A refused config is about the file's owner and mode; `config_file_refused`
  never means a folder is missing from `roots`, and widening `roots` (or setting `UMX_ROOTS`) will not clear it.
  Change `roots` only when the human asks for another folder (usage 031).
- Don't assume chmod 600 fixes every permission error. Follow the repair the refusal names: "not owned by you"
  needs `chown`, not chmod. umx names a refusal only when the file's owner or mode breaks its rules. A file the
  owner simply cannot read (0000, for example), a folder on the way that you cannot enter, or a denial after the
  file was opened stays a plain io error (`write_failed` for config). Look at `ls -l` on the file and its
  folders.
- Don't loosen the mode to "fix" a read: umx refuses a config writable by group or others, and a secrets or
  state file readable by them.

## Example
```bash
umx doctor
chmod 600 ~/.umx/config.toml   # the file the refusal names
umx daemon restart             # machine config only
umx config show
```

## Why
Since F49, umx judges these files on the handle it opens. A file the owner cannot open fails before that
judgment, so the specific refusal and its repair were lost (found in review). umx now looks at the path only to name
the refusal; it never reads a file on the strength of that look.

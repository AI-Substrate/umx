---
id: 142
title: Install or update the /umx skill with the pinned line umx doctor prints
route: start
status: superseded
source: F94 (security after F77, the project manager 2026-11-12), plan 112
reqs: [req-70, req-85]
supersedes: 117
superseded_by: 207
---
## Do

Install the skill once per machine, for every agent harness, and update it whenever umx doctor says so:

- Teammates (no umx checkout): run `umx doctor`. Its skill row prints the exact install line, pinned to the
  installer version and to the commit your umx was built from:
  `npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*'`. Run that line. It installs
  exactly the skill doctor compares with. It works through your gh or git login, since the repo is private, and
  it asks before it installs (no `-y`).
- In a umx checkout: `just skill-link` in the main checkout. It links `~/.agents/skills/umx` to the checkout's
  `skills/umx`, and the Claude, Codex and Pi skills folders to that. Rerunning it changes nothing.

Doctor's skill row compares `~/.agents/skills/umx` with the skill this umx was built with, offline:

- `doctor_skill_out_of_date` (a copy, exit 5): run the pinned line it prints. If your umx is older than the
  skill, update umx too.
- `doctor_skill_link_out_of_date` (a link, exit 5): in the checkout umx is built from, run `just skill-link`,
  then `just dev`.
- `doctor_skill_missing` (information, exit 0): no skill is installed; run the pinned line it prints.

## Don't

Don't use an unpinned `npx skills add` line, or `-y`, in agent work: the README's line is for a person who has
read it. Don't hand-edit the installed skill (doctor then reports it out of date). Don't run `just skill-link` in
a linked worktree unless every agent on the machine should use that branch's skill (it refuses without
`--from-worktree`). If doctor's line has no `#<commit>`, your build did not know its commit or its skills/umx was
not as committed. If the commit is one you never pushed, GitHub does not have it and the install fails: use the
line without `#<commit>` then.

## Example

    umx doctor        # the skill row is ok, or prints the pinned line to install or update it
    npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*'

## Why

An unpinned line runs whatever installer version npm serves today and installs whatever the default branch holds,
which may not be the skill this umx was built with (security, F94). Pinning both makes the install the exact
skill doctor checks. The commit cannot be written into a doc (a file cannot hold the commit it is committed in, and
a fixed one goes stale), so doctor prints it from the build.

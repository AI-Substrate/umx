---
id: 117
title: Install the /umx skill for every harness, and let umx doctor say when it is out of date
route: start
status: superseded
source: F77 (the operator via the project manager, 2026-11-11), plan 087
reqs: [req-70, req-85]
supersedes: none
superseded_by: 142
---
## Do

Install the skill once per machine, for every agent harness:

- Teammates (no umx checkout): `npx skills add AI-Substrate/umx --skill umx -g --agent '*' -y`. It works
  through your gh or git login, since the repo is private. It copies the skill into `~/.agents/skills/umx` and
  links each harness's skills folder to it. Run the same line again to update.
- In a umx checkout: `just skill-link` in the main checkout. It links `~/.agents/skills/umx` to the checkout's
  `skills/umx`, then links `~/.claude/skills/umx`, `~/.codex/skills/umx` (to `../../.agents/skills/umx`) and
  `~/.pi/agent/skills/umx` (to `../../../.agents/skills/umx`). Rerunning it changes nothing.

Run `umx doctor` after updating umx. Its `skill` row compares `~/.agents/skills/umx` with the skill this umx was
built with, offline:

- `doctor_skill_out_of_date` (a copy, exit 5): rerun the npx line above. If your umx is older than the skill,
  update umx too.
- `doctor_skill_link_out_of_date` (a link, exit 5): the checkout's skill and this binary differ. In the checkout
  umx is built from, run `just skill-link`, then `just dev`.
- `doctor_skill_missing` (information, exit 0): no skill is installed; install it as above.

## Don't

Don't hand-edit the installed skill: any change makes doctor report it out of date. Don't run `just skill-link`
in a linked worktree unless you mean every agent on the machine to use that branch's skill (it refuses without
`--from-worktree`). A harness folder that is a real folder, not a link, is left alone with a warning: move it
away yourself if you want the link.

## Example

    npx skills add AI-Substrate/umx --skill umx -g --agent '*' -y
    umx doctor        # the skill row is ok, or names the command to update it

## Why

Agents learn umx's verbs from the skill. A skill older or newer than the binary teaches commands that refuse or
miss new ones, which looks like a broken umx. The skill cannot record the commit it is committed in, so umx
compares a fingerprint of the skill's files (dotfiles such as `.DS_Store` skipped) taken when it was built.

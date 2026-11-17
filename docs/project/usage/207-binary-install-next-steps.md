---
id: 207
title: After a binary install, run the service and skill lines umx doctor prints; build the graph view from source
route: start
status: current
source: D76a Q10 and Q11 (2026-11-17), workstream 012 s9
reqs: [req-70, req-85]
supersedes: 142
---
## Do

Right after umx is installed as a binary (no umx checkout), run `umx doctor` and run the commands it prints:

- `doctor_service_none` (information): its next is `umx daemon install`. Run it so the daemon starts at login.
- `doctor_skill_missing` (information) or `doctor_skill_out_of_date` (a copy, exit 5): run the install line the
  message prints, as printed. It is pinned to the installer version and to the commit your umx was built from, and
  it carries `-y`, so it runs without a prompt:
  `npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y`. It installs exactly the skill
  doctor compares with, through your gh or git login.
- `doctor_skill_link_out_of_date` (a link, exit 5): in the checkout umx is built from, run `just skill-link`,
  then `just dev`.
- In a umx checkout, `just skill-link` in the main checkout instead of the npx line. Rerunning it changes nothing.

The graph view (`umx ui`) ships only with a source build. In a binary install `umx ui` refuses with
`ui_dir_refused`, saying the folder it was built from is not on this machine, and prints the commit this umx was
built from. To get the view: clone `https://github.com/AI-Substrate/umx`, check out the commit `umx version` reports
(`git -C umx checkout <commit>`), build its `apps/ui` (`umx docs get ui`), then point umx at it with `UMX_UI_DIR` or
`umx config set ui.dir <clone>/apps/ui`. If umx does not know its commit, use the source it was built from, or
update umx first. After updating umx, check out its new commit and build again.

## Don't

Don't type your own `npx skills add` line: an unpinned one installs whatever the default branch holds today. If
doctor's line has no `#<commit>`, your build did not know its commit or its skills/umx was not as committed. If the
commit is one that was never pushed, GitHub does not have it and the install fails: use the line without
`#<commit>` then. Don't hand-edit the installed skill (doctor then reports it out of date). Don't run `just
skill-link` in a linked worktree unless every agent on the machine should use that branch's skill. Don't retry
`umx ui` in a binary install, and don't look for a graph view download: there is none. Don't build the view from
the default branch: the view speaks one protocol and the daemon refuses any other (`protocol_mismatch`), so a newer
checkout can build a view your daemon rejects.

## Example

    umx doctor                       # prints the next commands
    umx daemon install
    npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y
    git clone https://github.com/AI-Substrate/umx.git ~/src/umx
    git -C ~/src/umx checkout <commit>   # the commit umx version reports
    pnpm --dir ~/src/umx/apps/ui install --frozen-lockfile && pnpm --dir ~/src/umx/apps/ui build
    umx config set ui.dir ~/src/umx/apps/ui

## Why

Install scripts put umx on `PATH` and start the daemon but only print the service and skill steps; agents run
them, and agents follow doctor (D76a Q10). The line used to leave out `-y`, so the installer stopped to ask and an
agent could not run it; pinning the installer and the commit is what makes the install the exact skill doctor
checks (security, F94), so `-y` confirms nothing else. Releases ship binaries only and the view stays a source
build (D76a Q11), so a binary install needs a hint for how to build it, pinned to the binary's commit because the
view and the daemon must speak the same protocol. Supersedes note 142, which said to leave out `-y`.

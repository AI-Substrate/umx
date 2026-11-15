---
id: 171
title: Keep a view up for a person with umx ui --detach, check it with umx ui status, end it with umx ui stop, and bring it forward with umx ui open --raise
route: explore
status: current
source: workstream 009 s10 M1b2, plan 161 (the project manager, 2026-11-12; design agreed by the architect and security)
reqs: []
supersedes: none
---
## Do

- To give a person a view that outlives your shell, run `umx ui --detach --graph alpha,beta`. It returns once the
  view is up, with `url` (no token), `port`, `pid`, `graphs` and `detached: true`, and opens their browser. Add
  `--no-open` to start it without opening anything (JSON is fine: a detached run never prints a link).
- To bring the view forward later, run `umx ui open --graph beta --raise --note "<why>"`. If a tab is open it marks
  its title and asks for focus (the title mark is certain; coming to the front is up to the browser, so don't promise
  it); if none is, umx opens one with a single-use link. Every such link works once, within
  60 s. This is how you "reopen" the view: you never hold a link.
- Check whether a view runs with `umx ui status`: `running`, `port`, `pid` and `started`, never which graphs it shows.
- End it with `umx ui stop` (`stopped: true`, or `false` when none ran).
- On `ui_already_running` there is already a view for this UMX_HOME: run its next (`umx ui open --graph .. --raise`
  to show your graphs in it, or `umx ui status`), or `umx ui stop` first if you really want a fresh one.
- On `ui_no_view` from any `umx ui` command, run its next: `umx ui --detach --graph <alias>` when nothing runs, or
  `umx ui open --graph <alias> --raise` when a view runs but no tab is open.

## Don't

- Don't use `--raise` by habit: taking the person's focus is the most intrusive thing you can do. Raise when they
  asked to see something now.
- Don't expect a view to survive the daemon: `umx daemon restart` (or a daemon that stops answering for about 5 s, or
  a daemon upgrade) closes it, and a detached view with no tab open stops by itself after 30 minutes. Start it again
  with `umx ui --detach`.
- Don't run a second `umx ui` for the same UMX_HOME, in a terminal or detached: one runs at a time (this changed in
  plan 161; before, a terminal `umx ui` could run beside another).
- Don't look for the link's token anywhere: no envelope, status or file holds it. `--no-open --human` (a terminal
  run for a person) is the only thing that prints it.

## Example

    umx ui --detach --graph alpha,beta
    umx ui open --graph gamma --raise --note "the decision you asked about"
    umx ui status
    umx ui stop

## Why

`umx ui` held a foreground process and never printed its link, so a coordinating agent had to keep a shell alive for
the person and could not hand over a way back to the view. The view now runs detached, tied to the daemon, and an
agent reopens it with `--raise`, which hands the browser a fresh one-use link and never gives the agent one.

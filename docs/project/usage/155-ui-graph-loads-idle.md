---
id: 155
title: umx ui --graph loads a graph that unloaded when idle, and the view shows who sent each ui command
route: explore
status: current
source: workstream 009 Rust follow-ups, plan 129 (S2 follow-up c; architect FU2 and security N3)
reqs: [req-104, req-15]
supersedes: none
---
## Do

Run `umx ui --graph <alias>` for any graph this machine remembers as loaded, even one the daemon unloaded after
it sat idle. umx loads it again first, as any command naming it would, then starts the view. No `umx graph load`
beforehand.

When `umx ui --graph <alias>` is refused with `graph_not_loaded`, run the command in its `next`: `umx graph load
<folder> --as <alias>` for a graph someone unloaded on purpose, `umx graph list` for an alias this machine does
not know.

Expect the view to label each `umx ui focus|highlight|filter|fit|clear` with who sent it (your seat, else your
role, else your user), from the identity umx already records for your writes.

## Don't

Don't run `umx graph load` just to open the view on a graph that unloaded when idle: `umx ui --graph` does it.

Don't put who you are in `--note`: the view already shows it, and a note is for what to look at.

## Example

    umx ui --graph acme
    umx ui focus 01JC8XA4H7 --graph acme --note "start here"

If `acme` was unloaded with `umx graph unload acme`, the first command is refused:

    {"command":"ui","status":"error","errors":[{"code":"graph_not_loaded","message":"Graph acme is not loaded: it was unloaded.","next":"umx graph load /work/acme --as acme"}],...}

## Why

Before plan 129, `umx ui --graph` refused any graph not loaded right now, so a graph that went idle needed a
manual load first, unlike every other named read. A graph unloaded on purpose still stays unloaded until someone
loads it. Who sent a view command is a claim, the same one a write records, and its session is only ever a hash.

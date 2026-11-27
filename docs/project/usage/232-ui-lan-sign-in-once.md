---
id: 232
title: Share the graph view on a trusted LAN; each browser signs in once, until the password is rotated
route: start
status: current
source: LAN view fix, 2026-11-27 (project lead: the LAN view must ask for the password once)
reqs: [req-104]
supersedes: 228
---
## Do
Use `umx ui --lan --port 4748` only when the user asks to share the view on a trusted LAN.
Have the owner run `umx ui lan-password` privately and give the password to intended viewers.
Expect each browser to sign in once: its session lasts 30 days, across view, daemon and browser restarts.
Use `umx ui lan-password --rotate` to change the password and end every LAN session; no restart is needed.
Read `umx docs get ui` for the HTTP risk and access boundary.

## Don't
Do not enable LAN access by default, expose it to the internet, or copy password output into logs or transcripts.
Do not treat secured graphs as hidden from an authenticated LAN viewer.
Do not restart the view or daemon to sign viewers out: rotate the password.
Do not restart another user's running view or daemon to enable this mode.

## Example
    umx ui --lan --no-open --port 4748
    umx ui lan-password
    umx ui lan-password --rotate

## Why
LAN clients need no bridge or tunnel. A persistent owner-only password file and 30-day session cookies, kept as
hashes in an owner-only file tied to that password file, limit access without changing loopback behavior or asking
again after every restart. HTTP offers no confidentiality: the password, session cookie and graph data can be
sniffed, and a password holder can open the same graphs as a local viewer.

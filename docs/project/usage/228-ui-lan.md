---
id: 228
title: Share the graph view on a trusted LAN with a persistent password
route: start
status: current
source: LAN view request, 2026-11-26
reqs: [req-104]
supersedes: none
---
## Do
Use `umx ui --lan --port 4748` only when the user asks to share the view on a trusted LAN.
Have the owner run `umx ui lan-password` privately and give the password to intended viewers.
Read `umx docs get ui` for the HTTP risk and access boundary.
Use `umx ui lan-password --rotate`, then restart the view, to change the password and drop sessions.

## Don't
Do not enable LAN access by default, expose it to the internet, or copy password output into logs or transcripts.
Do not treat secured graphs as hidden from an authenticated LAN viewer.
Do not restart another user's running view or daemon to enable this mode.

## Example
    umx ui --lan --no-open --port 4748
    umx ui lan-password

## Why
LAN clients need no bridge or tunnel. A persistent owner-only password file and per-run session cookies limit
access without changing loopback behavior. HTTP offers no confidentiality: the password and graph data can be
sniffed, and a password holder can open the same graphs as a local viewer.

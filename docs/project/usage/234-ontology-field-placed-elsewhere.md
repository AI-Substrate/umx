---
id: 234
title: Declare a std field on a type std does not place it on; the add writes it
route: build
status: current
source: umx fixes batch 1, 2026-11-27 (project lead: add field transcript starts_on:date answered unchanged and declared nothing)
reqs: [req-100]
supersedes: none
---
## Do
In a std graph, declare a std field on a type std does not place it on, with std's kind:
`umx ontology add field transcript starts_on:date`. The reply has `"unchanged": false` and graph.json holds it.
Expect `"unchanged": true` only where std already gives the type the field (`starts_on` on `meeting`, `tags` on any
node) or the graph declared it before.

## Don't
Do not take an earlier `unchanged` for `starts_on` or `ends_on` on a non-dated type as declared: run the add again.
Do not give the field another kind (`std_field_conflict`).

## Example
    umx ontology add field transcript starts_on:date
    umx ontology add field meeting,transcript,person starts_on:date

## Why
std allows `starts_on` on any node but places it only on the dated types. A type has it in its declaration once the
graph declares it, so lists, columns and checks that read the declaration see it.

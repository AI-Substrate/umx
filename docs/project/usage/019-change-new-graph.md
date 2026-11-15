---
id: 019
title: Shape a new graph straight after creating it
route: build
status: current
source: D30 (2026-11-10); req-71
reqs: [req-71, req-07]
supersedes: none
---
## Do
`umx graph new` prints how to change the new graph's vocabulary and settings. Do that first: add the types, statuses and relations this problem needs, and set the title with `umx graph set <alias> --title`.

## Don't
Don't expect templates; there are none by design.

## Example
```bash
umx graph new ~/graphs/acme --alias acme
umx ontology add type constraint
umx graph set acme --title "Acme onboarding"
```

## Why
Templates were ruled out in favour of an obvious path to shape each graph.

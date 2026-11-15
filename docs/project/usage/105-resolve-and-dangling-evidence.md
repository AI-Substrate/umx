---
id: 105
title: umx resolve, and umx: evidence that names no node
route: explore
status: current
source: ws006 S0 (plan 076), graph-addressing-expert ws006 plan S0 (2026-11-11); workshop 002 s7.5, s7.9; ws006 S2 (plan 078, graph-addressing-expert F1)
reqs: [req-15, req-55, req-69]
supersedes: none
---
## Do
Run `umx resolve <address>` to see what an address names before you cite it or paste it into a document. It
accepts `umx:<alias>/<id>` with a full id or a prefix of 6 or more characters, `#<field>` for one field, and
`?repo=<git url>` as a hint. On `ok`, use the returned `address`, which carries the full id for stored nodes
and edges. Whole-graph addresses resolve to the virtual graph node and stay `umx:<alias>` (usage 118);
fragments on that virtual node are refused, including explicit graph-id fragments once visible metadata
identifies it. Evidence has the same graph-fragment boundary, with its own `evidence_pointer_invalid` code.
A copy of a graph is addressed by the alias it was loaded under; `umx:<alias>@<copy>/<id>` is planned
(usage 016, req-69), so resolve refuses it with usage_invalid and an evidence source in that form is kept as
`evidence_unchecked`.

When a write warns `dangling_evidence`, the pointer was stored but names no single node in that loaded graph:
find the node you meant (`umx node find <words> --graph <alias>`, or the candidates on an ambiguous prefix) and
add evidence that names it. `evidence_unchecked` means the cited graph is not loaded, or is secured; load it
if you want the pointer checked. A secured graph is never checked from another graph's write, even when it is
loaded: the pointer is kept as written, with no candidates and no full id. Run `umx resolve` on the address to
check it yourself.

## Don't
Don't expect `umx resolve` to clone a repo or read a passage. It reads loaded graphs; a graph the daemon cannot
load gives `graph_not_loaded`, and a selector (`#seq=`, `#L`, `#quote=`) comes back in canonical form (the evidence grammar, usage 107) with `read: false`; a selector evidence would refuse is refused here too.
Don't expect an evidence write to load the graph it cites: it never does, so an unloaded graph's source stays
`evidence_unchecked`. Don't treat an exit of 1 as a
crash: the code says why (`graph_not_loaded`, `id_ambiguous`, `node_not_found`, `copy_ambiguous`,
`field_not_found`).

## Example
```bash
umx resolve umx:demo/01JC8XA4
umx resolve umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7#problem
umx node add --type finding --title "Approval lag" --evidence 'umx:sec_b/01ZZZZZZZZZZZZZZZZZZZZZZZZ#quote=nothing here'
# stored, with warning dangling_evidence: no node has that id in sec_b
```

## Why
Evidence is provenance, so missing targets warn instead of refusing, but a pointer that silently names nothing is worse than one
that warns. Storing the full id means a short prefix cannot become ambiguous as the graph grows. A write that
loaded the graph it cites would be a side effect, and a probe into secured graphs, so evidence checks only read
what is loaded and open (a secured graph answers as if unloaded, d-20). Renamed aliases (`redirected`) need the
graph registry and come later.

---
id: 151
title: Check linked graphs together, so links into secured or named graphs are read
route: doctor
status: current
source: F113 (project lead, field use; security and graph-addressing expert rulings via the project manager, 2026-11-12), plan 123
reqs: [req-23, req-50, req-85]
supersedes: none
---
## Do

When `umx check --graph acme` shows `graph_not_loaded` rows and a `link_targets_unnamed` count, acme links into
graphs the check did not name. Name them all in one check:

    umx check --graph acme,acme_d1,acme_d2
    umx check --scope acme_all         # a saved scope names its members

Every graph listed is checked, and every one counts as named, so links between them are read: they come back clean,
or `edge_broken` when the node is gone. Each row has `graph` (the graph it was found in), and `data.graphs` gives
each graph's own `total`. `umx doctor` does this over every loaded graph by itself.

## Don't

Don't read `graph_not_loaded` on a single check as "that graph is not loaded": a secured or named-visibility graph
that the check did not name looks exactly the same, on purpose. Don't expect `link_targets_unnamed` to say which
graphs: it is a count, and `umx graph list` shows what is loaded. A listed graph that is not loaded is refused, as
`--graph x` alone is.

## Example

    umx check --graph acme                     # graph_not_loaded rows, link_targets_unnamed: 22 links
    umx check --graph acme,acme_d1,acme_d2     # the same links now read; graphs: acme 0, acme_d1 0, acme_d2 0
    umx doctor                                 # graph check ok for all three

## Why

Field use (F113): after a private graph's dependent graphs were set to named visibility (D53), `umx doctor` said that graph had
22 problems, and check refused a list, so a correctly secured setup looked broken. Links across graphs are reported
when broken (req-23), secured graphs stay out of requests that do not name them (req-50), and doctor must tell real
problems from settings (req-85).

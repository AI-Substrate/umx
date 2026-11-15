---
id: 173
title: Split two nodes that are not one thing with umx entity deny, and remove it with undeny
route: multi-graph
status: current
source: ws008 E2c, plan 139 (the project manager 2026-11-12; the graph-addressing expert's join order, security's who and coverage); reverses D63c item 8
reqs: [req-137]
supersedes: none
---
## Do
When a shared key or a link joins two nodes that are different things (two people with one name, a reused address), run `umx entity deny <a> <b> --reason "<why>"`. It stores one `not_same_as` edge with the reason in the first node's graph (or `--graph`, which must hold one of the two), and from then on no key or link joins the pair in any read that covers that graph and both nodes. `umx entity show` says which join it skipped (`entity_join_denied`) and why.

Remove a denial with `umx entity undeny <a> <b>`. To change its reason, undeny and deny again.

## Don't
Don't deny without a reason, or with a reason that holds a secret. Don't try to write `not_same_as` with `edge add`, `edge edit`, `edge delete` or a batch line: they are refused, so no denial exists without a reason. Don't deny a pair this graph links with `same_as`: unlink it first. Don't expect a denial to hide a node: each stays visible as its own entity. Don't expect a denial in a graph the read does not cover to count.

## Example
```bash
umx entity deny umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M --reason "two people named Jo Smith"
umx entity show umx:crm/01JC8XA4H7          # one member, entity_join_denied names the skipped key or link
umx entity undeny umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M
```

## Why
Keys and links are evidence that two nodes are one thing; sometimes the evidence is wrong and a person knows it. A denial records that knowledge once, with who said it and why, and every read applies it in the same fixed order, so every machine with the same graphs sees the same entities.

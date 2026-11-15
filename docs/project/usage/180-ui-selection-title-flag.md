---
id: 180
title: Read secret_suspected on umx ui selection and never repeat a flagged title
route: explore
status: current
source: ws009 plan 179 (the project manager 2026-11-13; security's ruling 2026-11-13 02:44, the F188 rule of usage 174)
reqs: [req-153, req-46]
supersedes: none
---
## Do
Read `secret_suspected` on each row `umx ui selection` returns. It is `["field:title"]` when umx's own secret check
flags that node's title, the same check `umx node get` and a write use, with this machine's allows. The row keeps
its title; the flag only names it. The `secret_suspected` column shows only when some row has it.

When a selected node's title is flagged, tell the person which node holds it (its address) and let them decide:
retitle it with `umx node edit`, or allow it with `umx embeddings allow --node <node> --field title` if they know it
is not a secret.

## Don't
Don't repeat, quote, summarise or copy a flagged title into a reply, a note, a highlight's `--note`, a log or another
graph: name the node by its address instead. Don't treat a row without `secret_suspected` as proof the title holds
no secret: it covers what umx's rules recognise, nothing more.

## Example
```bash
umx ui selection
# rows: {"address": "umx:demo/01JC8XA4H7...", "title": "...", "secret_suspected": ["field:title"]}
umx ui highlight umx:demo/01JC8XA4H7 --note "this node's title may hold a secret"
```

## Why
The person's selection names nodes the agent did not choose, so a title there can hold a secret the agent never saw
flagged elsewhere. The view itself shows a flagged title as its type and short id; agents get the same warning umx
gives on every other read, without umx echoing the value a second time.

---
id: 082
title: similar_to is umx's own relation; never add it by hand
route: build
status: current
source: ws004 s09, plan 037; the project manager and cypher-expert 2026-11-10
reqs: [req-65]
supersedes: none
---
## Do
To say two nodes are related, use a real relation (`supports`, `depends_on`, `part_of`, or one you declare). To see what umx found similar, read it: `umx neighbours <node> --relation similar_to`.

## Don't
Don't `umx edge add <a> similar_to <b>` or `umx ontology add relation similar_to`: both are refused with `name_invalid`, whose `next` points at `umx neighbours <node> --relation similar_to`. umx keeps similar_to edges itself, from the vectors.

## Example
```bash
umx edge add 01JC8XA4H7 supports 01JC8XB2KQ         # a real relation: fine
umx edge add 01JC8XA4H7 similar_to 01JC8XB2KQ       # refused: name_invalid
```

## Why
One name with one meaning: if agents could add similar_to by hand, a similar_to step would mix umx's rankings with hand-made claims. An older graph's stored edge named similar_to still loads and is listed apart, marked derived: false.

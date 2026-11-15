---
id: 015
title: similar_to edges are followed only when you name them
route: similarity
status: superseded
superseded_by: 096
source: D26 (2026-11-10); workshop 008 s9; req-65, req-27
reqs: [req-65, req-27]
supersedes: none
---
## Do
Ask for similarity explicitly: `MATCH (a)-[s:similar_to]->(b) WHERE s.score > 0.8`. Treat it as a ranking, not a fact; if a similarity is real, add a real edge (e.g. possibly_same_as).

## Don't
Don't expect untyped steps (`-[]-`) or degree counts to include similar_to.

## Example
```bash
MATCH (a:question)-[s:similar_to]->(b) WHERE s.score > 0.8 RETURN b.title
```

## Why
Similarity edges are many and machine-made; keeping them out by default keeps walks meaningful.

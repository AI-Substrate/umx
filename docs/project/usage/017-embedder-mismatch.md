---
id: 017
title: A pulled graph may use a different embedding model
route: build
status: current
source: D32 (2026-11-10); req-75
reqs: [req-75]
supersedes: none
---
## Do
On `embedder_mismatch`, keep working on the committed vectors and choose: configure a provider for the graph's model, or deliberately run `umx embeddings reembed --model <m> --dry-run` to see the cost before re-embedding.

## Don't
Don't re-embed a shared graph without agreeing it with its owners; it rewrites every vector line.

## Example
```bash
umx embeddings reembed --model text-embedding-3-small@256 --dry-run
```

## Why
Models never mix in one graph; re-embedding is deliberate.

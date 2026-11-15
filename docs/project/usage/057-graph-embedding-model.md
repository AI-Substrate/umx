---
id: 057
title: Record a graph's embedding model when you create it
route: build
status: current
source: D32 (2026-11-10); ws004 s06 part A, embeddings-expert and graph-storage-expert agreement 2026-11-10
reqs: [req-75, req-37]
supersedes: none
---
## Do
When a graph will use similarity search, give its embedding model at creation:
`umx graph new <path> --alias <alias> --embedding-model <model@dims>` (add `+p<N>` when the model uses a prompt
prefix, for example `qwen3-embedding-0.6b@256+p1`). The key is lower-case letters, digits, `.`, `_` and `-`, then
`@` and the width. umx writes it to graph.json's `embedding` section; `umx graph list` and `graph load` show it.

On `embedder_mismatch` after a load or pull, keep working: the graph keeps its own model, and vectors from two
models are never mixed. See note 017 for the choices.

## Don't
Don't expect umx to pick a model for a graph from whatever embedder is configured. A graph without a recorded
model stays without one until you choose.

Don't put a provider or deployment name in the key (`my-azure-deployment@256`). Use the model's own name, so two
deployments of one model share vectors.

## Example
```bash
umx graph new /Users/me/work/launch --alias launch --embedding-model text-embedding-3-small@256
umx graph list
```

## Why
Vectors are only comparable within one model and width (req-75). Recording the model in the committed graph.json
means every clone knows which model its vectors belong to.

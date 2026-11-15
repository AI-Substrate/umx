---
id: 068
title: Re-embed a graph with another model only when the operator asks, cost first
route: doctor
status: current
source: ws004 s06 part B (req-75, D32), 2026-11-10
reqs: [req-75, req-46]
supersedes: none
---
## Do
On `embedder_mismatch` (the graph's vectors are one model, the configured embedder makes another), show the cost
first. Its `next` does exactly that:
`umx embeddings reembed --graph <alias> --dry-run` prints the nodes and texts still to embed with the configured
model, the estimated tokens, both model keys and whether the embedder is local or hosted. It calls no embedder
and changes nothing. Give that to the human and ask before going further.

Only when the human says so: `umx embeddings reembed --graph <alias>`. umx records the configured model in
graph.json and queues every node that needs a vector in it. The embed worker then makes them in the background.
`umx graph list` shows `vectors_pending` falling to 0. Add `--model <model@dims>` to make sure it is the model you
mean: a different one is refused (`reembed_model_not_configured`).

While `vectors_pending` is above 0, similarity reports `similarity_incomplete`. The old model's vectors stay on disk,
so switching back to the old model later costs nothing to embed. The old model's folder stays until a person removes
it by hand (a prune verb is planned).

The other way out: configure an embedder for the graph's own model (`umx docs get config`). Then nothing is
re-embedded.

## Don't
Don't re-embed on your own because of a mismatch warning: it costs money with a hosted model and changes the
committed graph.json for everyone who pulls it. The `--dry-run` is free.

Don't expect a secured graph to be re-embedded with a hosted embedder: it is refused (`secured_embed_refused`), dry
run included, and nothing is sent.

## Example
```bash
umx embeddings reembed --graph launch --dry-run
umx embeddings reembed --graph launch --model text-embedding-3-small@256
umx graph list
```

## Why
Vectors from two models are never comparable, so a graph has one model (req-75). Re-embedding is the only way to
change it, and it costs money with a hosted model, so it happens only when asked, with the price shown first. The
old model's vectors stay in their own `embeddings/<model@dims>/` folder, never mixed in.

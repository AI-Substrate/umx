---
id: 092
title: A search waits at most 5 s for its query vector, then answers with word matches
route: explore
status: current
source: workstream 004 s08 (plan 058); bound agreed with the embeddings-expert and architect 2026-11-11
reqs: [req-53]
supersedes: none
---
## Do
Read `similarity_unavailable` on a `umx search` as "these are word and exact matches only", and keep working with
them. When its cause is "the query embed took over 5 s (the local model may still be loading ...)", search again in
a moment: the model finishes loading in the background, and loading a graph that uses a local model starts that load
early. A cause of "the query looks like it holds a secret" means the query was not sent to the hosted embedder; drop
the secret from the query.

## Don't
Don't read a word-only answer as "nothing similar exists", and don't retry in a tight loop: each search already waits
up to 5 s. Don't treat `umx embeddings probe` as bounded: it is the diagnostic and reports the real time, cold load
and hosted retries included.

## Example
```bash
umx search "approval delays"
# warning similarity_unavailable: Similarity was not used for every graph searched: the query embed took over 5 s ...
umx search "approval delays"   # a moment later, with the model warm
```

## Why
req-53: a slow or hung provider, or a cold local model, must never make search hang or look empty. The bound covers
any wait behind another search's embed, and background embedding pauses while a query waits, so a search during a
big ingest still gets its vector.

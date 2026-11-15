---
id: 069
title: Pending vectors are normal on a graph whose model is not configured here
route: explore
status: current
source: ws004 s06 part B (req-75, D32), 2026-11-10
reqs: [req-75]
supersedes: none
---
## Do
Read `vectors_pending` in `umx graph list` and `graph load`. It counts nodes whose vectors, in the graph's own model,
are missing or stale. With an embedder for that model it falls to 0 as the background worker catches up. With no
such embedder (a graph pulled from a teammate on another model), new and edited nodes stay pending, and that is
expected. Keep working. Word search works. Similarity between nodes uses the committed vectors of the graph's own
model and reports `similarity_incomplete` while any are pending; free-text similarity needs an embedder for that
model (once the s13 vector scan lands).

To clear them, follow usage 068: configure the graph's model, or ask the human about re-embedding.

## Don't
Don't treat pending vectors as an error, and don't re-embed to make the number go away.

`null` means the graph has no model yet and no embedder is configured. There is nothing to count.

## Example
```bash
umx graph list
```

## Why
umx never makes a graph's vectors with a different model than the one it records (D32). So until a matching
embedder is configured, changed nodes wait instead of being embedded wrongly.

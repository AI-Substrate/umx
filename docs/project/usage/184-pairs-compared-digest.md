---
id: 184
title: Key kept similarity pairs by the reply's own compared digests, not by a status read around the call
route: explore
status: current
source: ws004 s36 (F206, plan 186); embeddings-expert, graph-storage-expert, architect and security design round 2026-11-13
reqs: [req-136, req-65]
supersedes: none
---
## Do
When you keep `umx similarity pairs` rows between calls, key them by the reply's `compared` list: one
`{"graph", "vectors_digest"}` per compared graph, in `graphs` order. Each digest is the one `umx embeddings status`
gives for that graph, taken in the same step that read the graph's vectors, so it names the vectors those rows came
from. On a later call or status read, re-read the pairs only when a graph's digest differs from the one you keyed
them by. `--rows ids` and full rows give the same `compared`. This refines usage 182's caching advice; its
`--rows ids` advice stands.

## Don't
Don't key pairs by an `embeddings status` read made before or after the pairs call: an edit and its undo can both
land while the call runs, and status would show the same digest on both sides of a change the rows may carry. Don't
look for skipped graphs in `compared`: graphs in `skipped_model` and secured graphs you did not name are never listed,
not even as `null`. A secured graph you name is compared and listed with its digest (status still shows `null` for
it). Don't treat the digest as proof the vectors match: it is a cache key over the model key and each vector's node
id and text hash (usage 182).

## Example
```bash
umx similarity pairs --graph finance,ops --k 10 --limit 250000 --rows ids
# graphs: ["finance","ops"]
# compared: [{"graph":"finance","vectors_digest":"5b1f...e2"},{"graph":"ops","vectors_digest":"9c07...41"}]
umx embeddings status
# graphs: [{"graph":"finance","vectors_digest":"5b1f...e2", ...},{"graph":"ops","vectors_digest":"a3d2...0f", ...}]
# ops moved since the read: read the pairs again.
```

## Why
The graph view caches pairs per graph digest and read status after each pairs read. A change and its undo inside
one pairs read (A, B, A) looked unchanged, though the rows were read at B. The reply now states the digest of what
it actually compared (F206).

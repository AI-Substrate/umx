---
id: 073
title: An embedding model key names at most 8192 dims
route: build
status: current
source: round 2 (security verdict on ws004 s13, 2026-11-10)
reqs: [req-34, req-75]
supersedes: none
---
## Do
Name embedding models as `<model>@<dims>` with dims from 1 to 8192, for example `--embedding-model nomic-embed-text@256`. Every current model fits; the widest hosted one is 3072.

If `umx graph load` refuses a pulled graph with `graph_invalid` on graph.json, check its `embedding.key`. A key above 8192 dims is refused on purpose. Fix the key in graph.json to the model the graph was really embedded with, or ask whoever committed it.

## Don't
Don't name a key wider than 8192 dims. umx refuses it instead of sizing memory from it. A graph loads that space's committed vectors into memory, so an absurd width from a pulled file could exhaust the daemon's memory.

## Example
```
umx graph new ./g --alias g --embedding-model nomic-embed-text@256    # fine
umx graph new ./g --alias g --embedding-model x@100000                # refused: usage_invalid, "--embedding-model must be model@dims or model@dims+pN (dims 1 to 8192), ..."
```

## Why
From ws004 s13 on, a loaded graph holds the vectors of the key its graph.json records, and graph.json arrives by `git pull`. Capping dims keeps a small pulled file from making the daemon allocate gigabytes.
